# Contexto de build: a raiz do repositório, porque o receptor usa crates/hangar-telemetry pelo caminho.
#   docker build -f services/telemetry-ingest/Dockerfile .
FROM rust:1-trixie AS build
WORKDIR /src
COPY crates crates
COPY services/telemetry-ingest services/telemetry-ingest
WORKDIR /src/services/telemetry-ingest
# O Cargo.lock vem junto com a pasta do receptor.
RUN cargo build --release --locked

FROM debian:trixie-slim
COPY --from=build /src/services/telemetry-ingest/target/release/telemetry-ingest /usr/local/bin/telemetry-ingest
USER nobody
ENV LISTEN=0.0.0.0:8080
EXPOSE 8080
CMD ["telemetry-ingest"]
