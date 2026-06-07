# syntax=docker/dockerfile:1.7

FROM rust:1-slim AS builder
WORKDIR /build

# Cache dependencies first.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/deps/chime-* target/release/chime*

COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /build/target/release/chime /usr/local/bin/chime
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/chime"]
