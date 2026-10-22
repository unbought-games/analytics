FROM rust:1.99.0-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY examples/producers ./examples/producers
RUN cargo build --locked --release --package analytics-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/analytics/exports /var/lib/analytics/handoff /run/analytics/secrets \
    && chmod 700 /var/lib/analytics/handoff \
    && chown -R 65532:65532 /var/lib/analytics /run/analytics
COPY --from=build /src/target/release/analytics-server /usr/local/bin/analytics-server
USER 65532:65532
ENV ANALYTICS_EXPORT_DIRECTORY=/var/lib/analytics/exports ANALYTICS_SECRET_DIRECTORY=/run/analytics/secrets
EXPOSE 8080
ENTRYPOINT ["analytics-server"]
CMD ["serve", "--bind", "0.0.0.0:8080"]
