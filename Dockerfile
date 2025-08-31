# Build with a modern compiler (>=1.82)
FROM rust:1.84-bookworm AS build
WORKDIR /app
COPY Cargo.* ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && cargo fetch
COPY src ./src
# Optional but recommended if you add a Cargo.lock later:
# RUN cargo build --release --locked
RUN cargo build --release

# Run on slim Debian with CA certs for TLS
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
ENV UPSTREAM_BASE=https://cloud.r-project.org
ENV LISTEN_ADDR=0.0.0.0:8080
EXPOSE 8080
# non-root user
RUN useradd -u 10001 -r -s /usr/sbin/nologin appuser
USER appuser
COPY --from=build /app/target/release/smartcran-logger /smartcran-logger
ENTRYPOINT ["/smartcran-logger"]
