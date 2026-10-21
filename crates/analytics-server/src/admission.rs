//! Fail-fast application admission with independently enforced request/stream lifetimes.
use analytics_core::{ApiError, ApiResult};
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{Method, StatusCode, Version, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_MAX_IN_FLIGHT_REQUESTS: usize = 64;
pub const MAX_IN_FLIGHT_REQUESTS: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub struct AdmissionLimits {
    pub max_in_flight: usize,
    pub request_timeout: Duration,
    pub stream_timeout: Duration,
}
impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            max_in_flight: DEFAULT_MAX_IN_FLIGHT_REQUESTS,
            request_timeout: Duration::from_secs(30),
            stream_timeout: Duration::from_secs(300),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Admission {
    permits: Arc<Semaphore>,
    limits: AdmissionLimits,
}
impl Admission {
    #[cfg(test)]
    pub(crate) fn new(limit: usize) -> ApiResult<Self> {
        Self::with_limits(AdmissionLimits {
            max_in_flight: limit,
            ..Default::default()
        })
    }
    pub(crate) fn with_limits(limits: AdmissionLimits) -> ApiResult<Self> {
        if !(1..=MAX_IN_FLIGHT_REQUESTS).contains(&limits.max_in_flight) {
            return Err(ApiError::bad_request(
                "Request admission limit must be between 1 and 4096",
            ));
        }
        if limits.request_timeout.is_zero()
            || limits.request_timeout > Duration::from_secs(600)
            || limits.stream_timeout.is_zero()
            || limits.stream_timeout > Duration::from_secs(3600)
        {
            return Err(ApiError::bad_request(
                "Request timeout must be positive and at most 600s; stream timeout positive and at most 3600s",
            ));
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(limits.max_in_flight)),
            limits,
        })
    }
}

fn close_unread_request(response: &mut Response, version: Version) {
    if matches!(version, Version::HTTP_10 | Version::HTTP_11) {
        response.headers_mut().insert(
            header::CONNECTION,
            header::HeaderValue::from_static("close"),
        );
    }
}

pub(crate) async fn admit(
    State(admission): State<Admission>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == "/healthz" && matches!(*request.method(), Method::GET | Method::HEAD)
    {
        return next.run(request).await;
    }
    let version = request.version();
    let Ok(permit) = admission.permits.try_acquire_owned() else {
        let mut response = ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded",
            "Service request capacity is full; retry with bounded backoff",
        )
        .into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, header::HeaderValue::from_static("1"));
        // Do not unconditionally disable keep-alive for complete uploads.
        // Hyper cheaply drains a dropped body once or closes if still unread;
        // we never await rejected bodies or create a separate drain work queue.
        return response;
    };
    // Covers body extraction, authentication, pool waits and handler execution.
    // Cancelling the future on timeout drops the request body and local permit.
    let response =
        match tokio::time::timeout(admission.limits.request_timeout, next.run(request)).await {
            Ok(response) => response,
            Err(_) => {
                drop(permit);
                let mut response = ApiError::new(
                    StatusCode::REQUEST_TIMEOUT,
                    "request_timeout",
                    "Request body or processing deadline exceeded",
                )
                .into_response();
                close_unread_request(&mut response, version);
                return response;
            }
        };
    let (parts, body) = response.into_parts();
    if body.is_end_stream() {
        drop(permit);
        return Response::from_parts(parts, body);
    }
    Response::from_parts(
        parts,
        Body::new(AdmittedBody::new(
            body,
            permit,
            admission.limits.stream_timeout,
        )),
    )
}

struct StreamState {
    inner: Option<Pin<Box<Body>>>,
    permit: Option<OwnedSemaphorePermit>,
    waker: Option<Waker>,
    expired: bool,
    finished: bool,
}
impl StreamState {
    fn release(&mut self) {
        self.inner.take();
        self.permit.take();
        self.waker.take();
    }
}
struct AdmittedBody {
    state: Arc<Mutex<StreamState>>,
    watchdog: tokio::task::JoinHandle<()>,
}
impl AdmittedBody {
    fn new(body: Body, permit: OwnedSemaphorePermit, lifetime: Duration) -> Self {
        let state = Arc::new(Mutex::new(StreamState {
            inner: Some(Box::pin(body)),
            permit: Some(permit),
            waker: None,
            expired: false,
            finished: false,
        }));
        let deadline = tokio::time::Instant::now() + lifetime;
        let watched = state.clone();
        // At most one live watchdog per admitted streaming response. It owns no
        // wait queue and releases BOTH producer/body resources and the permit,
        // even when the HTTP transport stops polling because its reader stalls.
        let watchdog = tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            let waker = {
                let mut state = watched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.finished {
                    return;
                }
                state.expired = true;
                let waker = state.waker.take();
                state.release();
                waker
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        });
        Self { state, watchdog }
    }
}
impl Drop for AdmittedBody {
    fn drop(&mut self) {
        self.watchdog.abort();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .release();
    }
}
impl HttpBody for AdmittedBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let this = self.get_mut();
        let mut state = this
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.finished {
            return Poll::Ready(None);
        }
        if state.expired {
            state.finished = true;
            return Poll::Ready(Some(Err(axum::Error::new(io::Error::new(
                io::ErrorKind::TimedOut,
                "Response streaming deadline exceeded",
            )))));
        }
        state.waker = Some(context.waker().clone());
        let inner = state.inner.as_mut().expect("active stream has body");
        let frame = inner.as_mut().poll_frame(context);
        if matches!(frame, Poll::Ready(None) | Poll::Ready(Some(Err(_)))) || inner.is_end_stream() {
            state.finished = true;
            state.release();
            this.watchdog.abort();
        }
        frame
    }
    fn is_end_stream(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finished
    }
    fn size_hint(&self) -> SizeHint {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(body) = &state.inner {
            body.size_hint()
        } else {
            SizeHint::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router, middleware,
        routing::{get, post},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use std::{
        io,
        time::{Duration, Instant},
    };
    use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc};
    use tower::ServiceExt;

    fn request(method: Method, path: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{not valid json"))
            .unwrap()
    }

    fn wrap(app: Router, admission: Admission) -> Router {
        app.layer(middleware::from_fn_with_state(admission, admit))
    }

    async fn overloaded(app: &Router, method: Method, path: &str) {
        let response = tokio::time::timeout(
            Duration::from_millis(250),
            app.clone().oneshot(request(method, path)),
        )
        .await
        .expect("Overloaded requests must reject without joining a wait queue")
        .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"]["code"], "overloaded");
    }

    #[tokio::test]
    async fn admission_rejects_http_and_mcp_before_extraction_and_recovers_on_cancel() {
        let admission = Admission::new(1).unwrap();
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let entered_handler = entered.clone();
        let resume_handler = resume.clone();
        let app = wrap(
            Router::new()
                .route("/healthz", get(|| async { "alive" }))
                .route("/readyz", get(|| async { "ready" }))
                .route(
                    "/hold",
                    get(move || {
                        let entered = entered_handler.clone();
                        let resume = resume_handler.clone();
                        async move {
                            entered.notify_one();
                            resume.notified().await;
                            "done"
                        }
                    }),
                )
                .route(
                    "/mcp",
                    post(|Json(_): Json<Value>| async { Json(json!({})) }),
                )
                .route(
                    "/v1/admin/execute",
                    post(|Json(_): Json<Value>| async { Json(json!({})) }),
                ),
            admission.clone(),
        );
        let pending = tokio::spawn(app.clone().oneshot(request(Method::GET, "/hold")));
        entered.notified().await;
        assert_eq!(admission.permits.available_permits(), 0);
        let started = Instant::now();
        for _ in 0..20 {
            overloaded(&app, Method::POST, "/mcp").await;
            overloaded(&app, Method::POST, "/v1/admin/execute").await;
            overloaded(&app, Method::GET, "/readyz").await;
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/healthz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert_eq!(admission.permits.available_permits(), 1);
        // The invalid JSON is only parsed after admission capacity is restored.
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/mcp"))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        let response = app.oneshot(request(Method::GET, "/readyz")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        drop(response);
        assert_eq!(admission.permits.available_permits(), 1);
    }

    type StreamReceiver = mpsc::Receiver<Result<Frame<Bytes>, io::Error>>;

    struct ChannelBody {
        receiver: StreamReceiver,
    }
    impl HttpBody for ChannelBody {
        type Data = Bytes;
        type Error = io::Error;
        fn poll_frame(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
            self.receiver.poll_recv(context)
        }
    }

    async fn streaming_fixture() -> (
        Router,
        Admission,
        mpsc::Sender<Result<Frame<Bytes>, io::Error>>,
    ) {
        let admission = Admission::new(1).unwrap();
        let (sender, receiver) = mpsc::channel(2);
        let receiver = Arc::new(AsyncMutex::new(Some(receiver)));
        let app = wrap(
            Router::new()
                .route(
                    "/file",
                    get(move || {
                        let receiver = receiver.clone();
                        async move {
                            Body::new(ChannelBody {
                                receiver: receiver.lock().await.take().unwrap(),
                            })
                        }
                    }),
                )
                .route("/readyz", get(|| async { "ready" })),
            admission.clone(),
        );
        (app, admission, sender)
    }

    #[tokio::test]
    async fn admission_holds_stream_permit_until_completion_or_drop() {
        let (app, admission, sender) = streaming_fixture().await;
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/file"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(admission.permits.available_permits(), 0);
        overloaded(&app, Method::GET, "/readyz").await;
        let mut body = response.into_body();
        sender
            .send(Ok(Frame::data(Bytes::from_static(b"first chunk\n"))))
            .await
            .unwrap();
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            "first chunk\n"
        );
        assert_eq!(admission.permits.available_permits(), 0);
        overloaded(&app, Method::GET, "/readyz").await;
        drop(sender);
        assert!(body.frame().await.is_none());
        assert_eq!(admission.permits.available_permits(), 1);
        assert_eq!(
            app.oneshot(request(Method::GET, "/readyz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let (app, admission, _sender) = streaming_fixture().await;
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/file"))
            .await
            .unwrap();
        assert_eq!(admission.permits.available_permits(), 0);
        // A disconnected client drops the response body without draining it.
        drop(response);
        assert_eq!(admission.permits.available_permits(), 1);
        assert_eq!(
            app.oneshot(request(Method::GET, "/readyz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn admission_releases_stream_permit_on_body_error() {
        let (app, admission, sender) = streaming_fixture().await;
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/file"))
            .await
            .unwrap();
        let mut body = response.into_body();
        sender
            .send(Err(io::Error::other("controlled stream failure")))
            .await
            .unwrap();
        assert!(body.frame().await.unwrap().is_err());
        // Keep the errored body alive; error itself must release the slot.
        assert_eq!(admission.permits.available_permits(), 1);
        assert_eq!(
            app.oneshot(request(Method::GET, "/readyz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        drop(body);
    }

    fn fixture_state() -> analytics_core::AppState {
        // Admission is tested before any DB/auth work. This lazy pool is never
        // connected; database security behavior has separate real PostgreSQL tests.
        let db = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(
            sqlx::postgres::PgConnectOptions::new()
                .host("127.0.0.1")
                .port(1)
                .username("synthetic")
                .database("synthetic"),
        );
        analytics_core::AppState {
            db: db.clone(),
            auth: Arc::new(crate::auth::DatabaseAuthorization { db, oidc: None }),
            config: Arc::new(analytics_core::ServiceConfig {
                public_base_url: "http://127.0.0.1:8080".into(),
                export_directory: "unused-fixture-exports".into(),
                secret_directory: "unused-fixture-secrets".into(),
                max_body_bytes: 1024,
            }),
        }
    }

    #[tokio::test]
    async fn composed_router_shares_extension_stream_capacity_with_mcp_and_admin() {
        use analytics_core::{AppState, AuthContext, Extension, ToolDefinition};
        struct StreamingExtension {
            receiver: Arc<AsyncMutex<Option<StreamReceiver>>>,
        }
        #[async_trait::async_trait]
        impl Extension for StreamingExtension {
            fn name(&self) -> &'static str {
                "admission-test"
            }
            fn router(&self) -> Router<AppState> {
                let receiver = self.receiver.clone();
                Router::new().route(
                    "/fixture/stream",
                    get(move || {
                        let receiver = receiver.clone();
                        async move {
                            Body::new(ChannelBody {
                                receiver: receiver.lock().await.take().unwrap(),
                            })
                        }
                    }),
                )
            }
            fn tools(&self) -> Vec<ToolDefinition> {
                Vec::new()
            }
            async fn call(
                &self,
                _: &AppState,
                _: &AuthContext,
                _: &str,
                _: Value,
            ) -> ApiResult<Value> {
                Err(ApiError::bad_request("No tools on this streaming fixture"))
            }
        }
        let state = fixture_state();
        let (sender, receiver) = mpsc::channel(1);
        let extension = Arc::new(StreamingExtension {
            receiver: Arc::new(AsyncMutex::new(Some(receiver))),
        });
        let app = crate::router_with_admission(state, vec![extension], None, 1).unwrap();
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/fixture/stream"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        for (method, path) in [
            (Method::POST, "/mcp"),
            (Method::POST, "/v1/admin/execute"),
            (Method::GET, "/readyz"),
        ] {
            overloaded(&app, method, path).await;
        }
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/healthz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        drop(response);
        drop(sender);
        let ping = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
            .unwrap();
        let restored = app.oneshot(ping).await.unwrap();
        assert_eq!(restored.status(), StatusCode::UNAUTHORIZED);
        assert!(restored.headers().contains_key(header::WWW_AUTHENTICATE));
    }

    #[tokio::test]
    async fn unpolled_stream_deadline_drops_producer_and_releases_capacity() {
        let admission = Admission::with_limits(AdmissionLimits {
            max_in_flight: 1,
            request_timeout: Duration::from_secs(1),
            stream_timeout: Duration::from_millis(50),
        })
        .unwrap();
        let (sender, receiver) = mpsc::channel(1);
        let receiver = Arc::new(AsyncMutex::new(Some(receiver)));
        let app = wrap(
            Router::new().route(
                "/stream",
                get(move || {
                    let receiver = receiver.clone();
                    async move {
                        Body::new(ChannelBody {
                            receiver: receiver.lock().await.take().unwrap(),
                        })
                    }
                }),
            ),
            admission.clone(),
        );
        let response = app.oneshot(request(Method::GET, "/stream")).await.unwrap();
        assert_eq!(admission.permits.available_permits(), 0);
        // Keep the response allocated and NEVER poll its body until after expiry.
        tokio::time::timeout(Duration::from_secs(1), async {
            while admission.permits.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            sender.is_closed(),
            "deadline must drop the underlying producer, not just free its slot"
        );
        let mut body = response.into_body();
        assert!(
            body.frame().await.unwrap().is_err(),
            "expired stream must report failure, not a successful empty EOF"
        );
        assert!(body.frame().await.is_none());
    }

    #[tokio::test]
    async fn real_tcp_headers_only_mcp_requests_timeout_and_restore_all_slots() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let app = crate::router_with_admission_limits(
            fixture_state(),
            vec![],
            None,
            AdmissionLimits {
                max_in_flight: 64,
                request_timeout: Duration::from_secs(1),
                stream_timeout: Duration::from_secs(1),
            },
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut stalled = Vec::new();
        for _ in 0..64 {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            socket.write_all(format!("POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 1\r\n\r\n").as_bytes()).await.unwrap();
            stalled.push(socket);
        }
        let client = reqwest::Client::new();
        let base = format!("http://{address}");
        let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                let response = client
                    .post(format!("{base}/mcp"))
                    .json(&ping)
                    .send()
                    .await
                    .unwrap();
                if response.status() == StatusCode::SERVICE_UNAVAILABLE {
                    break;
                }
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all 64 headers-only requests should saturate admission before their deadline");
        assert_eq!(
            client
                .get(format!("{base}/healthz"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        tokio::time::timeout(Duration::from_secs(3),async {
            for mut socket in stalled {
                let mut response=Vec::new();
                socket.read_to_end(&mut response).await.unwrap();
                let response=String::from_utf8(response).unwrap();
                assert!(response.starts_with("HTTP/1.1 408"));
                assert!(response.contains("request_timeout"));
                assert!(response.to_lowercase().contains("connection: close"));
            }
        }).await.expect("slow bodies must receive bounded 408 and connection closure without sending their missing byte");
        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .json(&ping)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        server.abort();
    }

    #[tokio::test]
    async fn real_tcp_nonreading_download_releases_producer_at_stream_deadline() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use tokio::io::AsyncWriteExt;
        struct RepeatedBody {
            dropped: Arc<AtomicBool>,
            polls: Arc<AtomicUsize>,
            chunk: Bytes,
        }
        impl Drop for RepeatedBody {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
            }
        }
        impl HttpBody for RepeatedBody {
            type Data = Bytes;
            type Error = io::Error;
            fn poll_frame(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
                self.polls.fetch_add(1, Ordering::SeqCst);
                Poll::Ready(Some(Ok(Frame::data(self.chunk.clone()))))
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicUsize::new(0));
        let dropped_handler = dropped.clone();
        let polls_handler = polls.clone();
        let entered = Arc::new(Notify::new());
        let entered_handler = entered.clone();
        let admission = Admission::with_limits(AdmissionLimits {
            max_in_flight: 1,
            request_timeout: Duration::from_secs(1),
            stream_timeout: Duration::from_millis(150),
        })
        .unwrap();
        let app = wrap(
            Router::new()
                .route(
                    "/stream",
                    get(move || {
                        let dropped = dropped_handler.clone();
                        let polls = polls_handler.clone();
                        let entered = entered_handler.clone();
                        async move {
                            entered.notify_one();
                            Body::new(RepeatedBody {
                                dropped,
                                polls,
                                chunk: Bytes::from(vec![b'x'; 64 * 1024]),
                            })
                        }
                    }),
                )
                .route("/ready", get(|| async { "ready" })),
            admission.clone(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(1024).unwrap();
        let mut reader = socket.connect(address).await.unwrap();
        reader
            .write_all(format!("GET /stream HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        entered.notified().await;
        let client = reqwest::Client::new();
        assert_eq!(
            client
                .get(format!("http://{address}/ready"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        // Keep the TCP client OPEN and never read its headers or body.
        tokio::time::timeout(Duration::from_secs(2), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("stream producer must be dropped independently of the stalled socket reader");
        assert!(polls.load(Ordering::SeqCst) > 0);
        assert_eq!(admission.permits.available_permits(), 1);
        assert_eq!(
            client
                .get(format!("http://{address}/ready"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        drop(reader);
        server.abort();
    }

    #[tokio::test]
    async fn early_http1_overload_can_close_before_upload_finishes_with_or_without_close_header() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // A/B the exact rejection path: the test-only outer layer removes just
        // the explicit close header, leaving Hyper's unread-body handling intact.
        for explicit_close in [true, false] {
            let (base, admission, sender) = streaming_fixture().await;
            let held = base
                .clone()
                .oneshot(request(Method::GET, "/file"))
                .await
                .unwrap();
            let rejected = Arc::new(Notify::new());
            let signalled = rejected.clone();
            let app = base.layer(middleware::from_fn(move |request: Request, next: Next| {
                let signalled = signalled.clone();
                async move {
                    let mut response = next.run(request).await;
                    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
                        if explicit_close {
                            response.headers_mut().insert(
                                header::CONNECTION,
                                header::HeaderValue::from_static("close"),
                            );
                        } else {
                            response.headers_mut().remove(header::CONNECTION);
                        }
                        signalled.notify_one();
                    }
                    response
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let mut acknowledged = 0;
            let mut upload_transport_errors = 0;
            let mut wire_503 = 0;
            for _ in 0..8 {
                let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
                socket.write_all(format!("POST /readyz HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 65536\r\n\r\nx").as_bytes()).await.unwrap();
                tokio::time::timeout(Duration::from_secs(1), rejected.notified())
                    .await
                    .unwrap();
                // Model a client still sending a valid bounded upload after an
                // early response, without a concurrent response reader.
                tokio::time::sleep(Duration::from_millis(10)).await;
                let chunk = [b'x'; 8192];
                let mut remaining = 65535;
                let mut send_failed = false;
                while remaining > 0 {
                    let size = remaining.min(chunk.len());
                    if socket.write_all(&chunk[..size]).await.is_err() {
                        send_failed = true;
                        break;
                    }
                    remaining -= size;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                let mut response = Vec::new();
                let read =
                    tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut response))
                        .await
                        .unwrap();
                let saw_status = response.starts_with(b"HTTP/1.1 503");
                if saw_status {
                    wire_503 += 1;
                }
                if send_failed || read.is_err() {
                    upload_transport_errors += 1;
                } else {
                    assert!(saw_status, "completed rejection response must carry503");
                    acknowledged += 1;
                }
                // Every upload is accounted for, without calling a send/reset
                // failure an observed503 or an application acceptance.
                assert_eq!(admission.permits.available_permits(), 0);
            }
            assert_eq!(acknowledged + upload_transport_errors, 8);
            println!(
                "HTTP1 early rejection: explicit_close={explicit_close}, requests=8, clean503={acknowledged}, transport_unknown={upload_transport_errors},503_headers_observed_when_reading_after_upload={wire_503}"
            );
            drop(held);
            drop(sender);
            assert_eq!(admission.permits.available_permits(), 1);
            server.abort();
        }
    }

    #[tokio::test]
    async fn complete_small_overload_upload_can_reuse_http1_connection() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let (app, admission, sender) = streaming_fixture().await;
        let held = app
            .clone()
            .oneshot(request(Method::GET, "/file"))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut socket = BufReader::new(tokio::net::TcpStream::connect(address).await.unwrap());
        for _ in 0..2 {
            socket.get_mut().write_all(format!("POST /readyz HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes()).await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                let mut line = String::new();
                socket.read_line(&mut line).await.unwrap();
                assert!(line.starts_with("HTTP/1.1 503"));
                let mut length = None;
                loop {
                    line.clear();
                    socket.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    assert!(
                        !line.is_empty(),
                        "connection closed before complete response headers"
                    );
                    let lower = line.to_ascii_lowercase();
                    assert_ne!(lower.trim(), "connection: close");
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut body =
                    vec![0; length.expect("small JSON response must include content length")];
                socket.read_exact(&mut body).await.unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
                    "overloaded"
                );
            })
            .await
            .unwrap();
        }
        assert_eq!(admission.permits.available_permits(), 0);
        drop(socket);
        drop(held);
        drop(sender);
        server.abort();
    }

    #[test]
    fn admission_limits_are_explicit_and_bounded() {
        assert!(Admission::new(0).is_err());
        assert!(Admission::new(MAX_IN_FLIGHT_REQUESTS + 1).is_err());
        assert!(Admission::new(DEFAULT_MAX_IN_FLIGHT_REQUESTS).is_ok());
        assert!(
            Admission::with_limits(AdmissionLimits {
                request_timeout: Duration::ZERO,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            Admission::with_limits(AdmissionLimits {
                stream_timeout: Duration::ZERO,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            Admission::with_limits(AdmissionLimits {
                request_timeout: Duration::from_secs(601),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            Admission::with_limits(AdmissionLimits {
                stream_timeout: Duration::from_secs(3601),
                ..Default::default()
            })
            .is_err()
        );
    }
}
