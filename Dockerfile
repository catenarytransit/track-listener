FROM node:22-bookworm-slim AS frontend
WORKDIR /build
COPY web/package.json ./
RUN npm install --no-audit --no-fund
COPY web/ ./
RUN npm run build

FROM rust:bookworm AS backend
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config cmake curl ca-certificates bzip2 libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml ./
COPY src/ ./src/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates libstdc++6 libgomp1 libgcc-s1 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /build/target/release/track-listener /usr/local/bin/track-listener
COPY --from=frontend /build/dist ./web/dist
COPY config.docker.toml ./config.toml
EXPOSE 4582
ENTRYPOINT ["track-listener"]
CMD ["--config", "config.toml"]
