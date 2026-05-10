FROM rust:1.94-bookworm AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends capnproto \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY tools ./tools

RUN cargo build --release -p latlng-server

FROM debian:bookworm-slim AS runtime-base

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system latlng \
    && useradd --system --gid latlng --create-home latlng

RUN mkdir -p /etc/latlng /var/lib/latlng \
    && chown -R latlng:latlng /etc/latlng /var/lib/latlng

EXPOSE 7421 7422

ENTRYPOINT ["/usr/local/bin/latlng-server"]
CMD ["--config", "/etc/latlng/latlng.toml"]

FROM runtime-base AS runtime-prebuilt

ARG LATLNG_SERVER_BINARY=target/release/latlng-server

COPY ${LATLNG_SERVER_BINARY} /usr/local/bin/latlng-server

RUN chmod +x /usr/local/bin/latlng-server

USER latlng

FROM runtime-base AS runtime

COPY --from=builder /app/target/release/latlng-server /usr/local/bin/latlng-server

RUN chmod +x /usr/local/bin/latlng-server

USER latlng
