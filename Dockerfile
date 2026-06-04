# syntax=docker/dockerfile:1

# ---- builder ----
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p sigo-cli \
 && cp /app/target/release/sigo /usr/local/bin/sigo

# ---- runtime ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates python3 \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --uid 10001 sigo
USER sigo
ENV XDG_DATA_HOME=/home/sigo/.local/share \
    XDG_CONFIG_HOME=/home/sigo/.config
COPY --from=builder /usr/local/bin/sigo /usr/local/bin/sigo
ENTRYPOINT ["sigo"]
