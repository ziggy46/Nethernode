# syntax=docker/dockerfile:1
#
# NetherNode as a single static binary on an empty base image.
#   docker build -t ghcr.io/ziggy46/nethernode:latest .
# Run it with docker-compose.yml (it needs the Docker socket).

# The panel is plain JavaScript, so it's built once on the build machine's
# own platform even when the image is for another one.
FROM --platform=$BUILDPLATFORM node:26-alpine AS web
WORKDIR /src/web
RUN npm install -g pnpm@11.22.0
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1.96-alpine AS build
# aws-lc (TLS) builds C code; protoc compiles the API definitions.
RUN apk add --no-cache build-base cmake perl protobuf-dev ca-certificates
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY proto/ proto/
COPY .sqlx/ .sqlx/
COPY --from=web /src/web/dist web/dist
ENV SQLX_OFFLINE=true
# Limits parallel compile jobs on small machines: --build-arg JOBS=2
ARG JOBS
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p nethernode ${JOBS:+--jobs "$JOBS"} \
    && cp target/release/nethernode /nethernode

FROM scratch
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build /nethernode /nethernode
ENV NETHERNODE_DATA__DIR=/data \
    NETHERNODE_HTTP__BIND=0.0.0.0:8080
VOLUME ["/data"]
EXPOSE 8080 25565
ENTRYPOINT ["/nethernode"]
CMD ["serve"]
