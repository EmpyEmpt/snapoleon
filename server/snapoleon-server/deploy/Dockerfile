# syntax=docker/dockerfile:1
#
# Build from the repository root (the server depends on workspace crates and shared assets):
#   docker build -f server/snapoleon-server/deploy/Dockerfile -t snapoleon-server .

FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends musl-tools clang cmake \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-unknown-linux-musl
WORKDIR /src
COPY . .
# Static musl binary so the runtime image needs no libc. The bundled SQLite and
# aws-lc are compiled with musl-gcc.
ENV CC_x86_64_unknown_linux_musl=musl-gcc
RUN cargo build --release --locked -p snapoleon-server --target x86_64-unknown-linux-musl \
 && mkdir /data

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /src/target/x86_64-unknown-linux-musl/release/snapoleon-server /snapoleon-server
COPY --from=build --chown=nonroot:nonroot /data /data
ENV SNAPOLEON_BIND=0.0.0.0:8787 \
    SNAPOLEON_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 8787
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/snapoleon-server", "healthcheck"]
ENTRYPOINT ["/snapoleon-server"]
