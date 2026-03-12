# Stage 1: Build
FROM rust:1.82-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
COPY templates/ templates/

RUN cargo build --release --bin mcpolly --bin mcpolly_mcp

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false mcpolly

COPY --from=builder /app/target/release/mcpolly /usr/local/bin/mcpolly
COPY --from=builder /app/target/release/mcpolly_mcp /usr/local/bin/mcpolly_mcp

RUN mkdir -p /data && chown mcpolly:mcpolly /data

USER mcpolly

ENV PORT=3000
ENV DATABASE_URL=/data/mcpolly.db
ENV RUST_LOG=mcpolly=info

EXPOSE 3000

VOLUME ["/data"]

ENTRYPOINT ["mcpolly"]
