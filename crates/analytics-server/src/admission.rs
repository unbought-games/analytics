//! Fail-fast application admission, including the lifetime of streaming responses.
use analytics_core::{ApiError, ApiResult};
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_MAX_IN_FLIGHT_REQUESTS: usize = 64;
pub const MAX_IN_FLIGHT_REQUESTS: usize = 4096;

#[derive(Clone)]
pub(crate) struct Admission {
    permits: Arc<Semaphore>,
}
impl Admission {
    pub(crate) fn new(limit: usize) -> ApiResult<Self> {
        if !(1..=MAX_IN_FLIGHT_REQUESTS).contains(&limit) {
            return Err(ApiError::bad_request(
                "Request admission limit must be between 1 and 4096",
            ));
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(limit)),
        })
    }
}

pub(crate) async fn admit(
    State(admission): State<Admission>,
    request: Request,
    next: Next,
) -> Response {
    // Liveness performs no database, authorization or module work. Readiness is
    // deliberately subject to admission because it acquires a database connection.
    if request.uri().path() == "/healthz" && matches!(*request.method(), Method::GET | Method::HEAD)
    {
        return next.run(request).await;
    }
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
        return response;
    };
    // Until headers exist the permit is owned by this future, so dropping a
    // cancelled handler releases it. Then transfer ownership into the body.
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    if body.is_end_stream() {
        drop(permit);
        return Response::from_parts(parts, body);
    }
    Response::from_parts(
        parts,
        Body::new(AdmittedBody {
            inner: Box::pin(body),
            permit: Some(permit),
        }),
    )
}

struct AdmittedBody {
    inner: Pin<Box<Body>>,
    permit: Option<OwnedSemaphorePermit>,
}
impl HttpBody for AdmittedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let frame = this.inner.as_mut().poll_frame(context);
        if matches!(frame, Poll::Ready(None) | Poll::Ready(Some(Err(_))))
            || this.inner.is_end_stream()
        {
            this.permit.take();
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
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
    use tokio::sync::{Mutex, Notify, mpsc};
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
        let receiver = Arc::new(Mutex::new(Some(receiver)));
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

    #[tokio::test]
    async fn composed_router_shares_extension_stream_capacity_with_mcp_and_admin() {
        use analytics_core::{AppState, AuthContext, Extension, ServiceConfig, ToolDefinition};
        struct StreamingExtension {
            receiver: Arc<Mutex<Option<StreamReceiver>>>,
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
        // Admission is tested before any DB/auth work. This lazy pool is never
        // connected; database security behavior has separate real PostgreSQL tests.
        let db = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(
            sqlx::postgres::PgConnectOptions::new()
                .host("127.0.0.1")
                .port(1)
                .username("synthetic")
                .database("synthetic"),
        );
        let state = AppState {
            db: db.clone(),
            auth: Arc::new(crate::auth::DatabaseAuthorization { db, oidc: None }),
            config: Arc::new(ServiceConfig {
                public_base_url: "http://127.0.0.1:8080".into(),
                export_directory: "unused-fixture-exports".into(),
                secret_directory: "unused-fixture-secrets".into(),
                max_body_bytes: 1024,
            }),
        };
        let (sender, receiver) = mpsc::channel(1);
        let extension = Arc::new(StreamingExtension {
            receiver: Arc::new(Mutex::new(Some(receiver))),
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

    #[test]
    fn admission_limits_are_explicit_and_bounded() {
        assert!(Admission::new(0).is_err());
        assert!(Admission::new(MAX_IN_FLIGHT_REQUESTS + 1).is_err());
        assert!(Admission::new(DEFAULT_MAX_IN_FLIGHT_REQUESTS).is_ok());
    }
}
