# Static musl binary, then a distroless image: CA certificates, no shell. It starts as root only to
# hand /data to PUID:PGID (default 65532, distroless's nonroot), then the binary drops root for good
# before opening anything (src/privileges.rs).
FROM rust:1.98-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app

# Dependencies first, so that a code change does not rebuild them.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY migrations migrations
COPY templates templates
COPY static static
COPY src src
RUN touch src/main.rs src/lib.rs && cargo build --release --locked \
    && mkdir /data

FROM gcr.io/distroless/static-debian12
COPY --from=builder /app/target/release/uptime-monitoring /usr/local/bin/uptime-monitoring
COPY --from=builder --chown=65532:65532 /data /data
ENV DATABASE_URL=sqlite:///data/uptime.db \
    BIND_ADDR=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s \
    CMD ["/usr/local/bin/uptime-monitoring", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/uptime-monitoring"]
