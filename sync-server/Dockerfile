# Static musl build of sync-server. Build context is this directory:
#   docker build -t readest-sync-server .
# The binary is fully static (musl + bundled SQLite), so the runtime image
# stays tiny and runs anywhere — including the scratch-like alpine base.
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM alpine:3.21
RUN adduser -D -H -u 1500 readest \
    && mkdir -p /data \
    && chown readest /data
COPY --from=build /build/target/release/sync-server /usr/local/bin/sync-server
ENV DATA_DIR=/data
# Portable defaults apply here too: the auto-generated jwt_secret.key lands in
# /data, so mounting a volume keeps logins valid across container upgrades.
VOLUME /data
EXPOSE 8080
USER readest
ENTRYPOINT ["/usr/local/bin/sync-server"]
