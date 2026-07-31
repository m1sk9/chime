# syntax=docker/dockerfile:1.26

FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /build

# aws-lc-sys (reqwest rustls backend) needs cmake + make to build its C sources;
# musl-tools provides the musl C toolchain for the static target.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake make musl-tools \
    && rm -rf /var/lib/apt/lists/*

# Pin the toolchain before adding the target: rust-toolchain.toml selects `stable`,
# which differs from the cargo-chef image's default, so the musl std must be
# installed against the resolved toolchain (otherwise the later build re-resolves
# and can't find std for the target).
COPY rust-toolchain.toml .

# Map Docker's TARGETARCH to the Rust musl target so the binary links statically
# against musl and carries no glibc dependency at runtime (see #15).
ARG TARGETARCH
RUN case "$TARGETARCH" in \
        amd64) target="x86_64-unknown-linux-musl" ;; \
        arm64) target="aarch64-unknown-linux-musl" ;; \
        *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && echo "$target" > /tmp/target \
    && rustup target add "$target"

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --target "$(cat /tmp/target)" --recipe-path recipe.json
COPY . .
RUN cargo build --release --target "$(cat /tmp/target)" \
    && cp "target/$(cat /tmp/target)/release/chime" /build/chime

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=builder /build/chime /usr/local/bin/chime
LABEL org.opencontainers.image.source=https://github.com/m1sk9/chime
USER nonroot:nonroot
# distroless has no shell, so the healthcheck must use exec-form.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/chime", "health"]
ENTRYPOINT ["/usr/local/bin/chime"]
