# Contexto de build: a raiz do repositório, porque o receptor usa crates/hangar-telemetry pelo caminho
# e o painel usa os logos dos agentes do desktop nativo.
#   docker build -f services/telemetry-ingest/Dockerfile .
FROM node:24-trixie-slim AS panel
WORKDIR /src
COPY desktop-native/assets/providers desktop-native/assets/providers
COPY services/telemetry-ingest/panel/package.json services/telemetry-ingest/panel/package-lock.json services/telemetry-ingest/panel/
WORKDIR /src/services/telemetry-ingest/panel
RUN npm ci --no-audit --no-fund
COPY services/telemetry-ingest/panel ./
RUN npm run build

FROM rust:1-trixie AS build
WORKDIR /src
COPY crates crates
COPY services/telemetry-ingest services/telemetry-ingest
# O executável embute o build do painel (rust-embed lê panel/dist na compilação).
COPY --from=panel /src/services/telemetry-ingest/panel/dist services/telemetry-ingest/panel/dist
WORKDIR /src/services/telemetry-ingest
# O Cargo.lock vem junto com a pasta do receptor.
RUN cargo build --release --locked

FROM debian:trixie-slim
COPY --from=build /src/services/telemetry-ingest/target/release/telemetry-ingest /usr/local/bin/telemetry-ingest
USER nobody
ENV LISTEN=0.0.0.0:8080
EXPOSE 8080
CMD ["telemetry-ingest"]
