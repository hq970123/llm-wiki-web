# syntax=docker/dockerfile:1
#
# LLM Wiki — self-hosted single-account web version (one container).
#
#   docker build -t llm-wiki-web .
#   docker run -p 7860:7860 -v llm-wiki-data:/data llm-wiki-web
#
# Works as a Hugging Face Docker Space (port 7860, uid 1000, /data volume).

############################ 1) frontend (Vite, web mode) ############################
FROM node:22-bookworm-slim AS web
WORKDIR /src
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY index.html vite.config.ts tsconfig.json tsconfig.app.json tsconfig.node.json components.json ./
COPY src ./src
COPY public ./public
RUN npm run build:web

############################ 1b) MCP server (Node, remote MCP over HTTP) ############################
FROM node:22-bookworm-slim AS mcp
WORKDIR /mcp
COPY mcp-server/package.json mcp-server/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY mcp-server/tsconfig.json ./
COPY mcp-server/src ./src
COPY mcp-server/test ./test
RUN npm run build && npm prune --omit=dev

############################ 2) backend (Rust, axum server) ############################
FROM rust:1-bookworm AS server
RUN apt-get update \
 && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev cmake clang pkg-config mold \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The server compiles the upstream backend sources (src-tauri/src) against
# Tauri shims; file_sync.rs include_str!s one JSON file from src/lib.
COPY src-tauri/src ./src-tauri/src
COPY src/lib/source-watch-defaults.json ./src/lib/source-watch-defaults.json
COPY server ./server
WORKDIR /src/server
# mold links the (large: lancedb/datafusion) binary much faster and with
# far less memory than GNU ld.
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"
ARG BUILD_JOBS=
RUN if [ -n "$BUILD_JOBS" ]; then export CARGO_BUILD_JOBS="$BUILD_JOBS"; fi; \
    cargo build --release --locked --bin llm-wiki-web \
 && cp target/release/llm-wiki-web /llm-wiki-web

############################ 3) pdfium for the target arch ############################
FROM debian:bookworm-slim AS pdfium
ARG TARGETARCH
COPY src-tauri/pdfium/libpdfium.so src-tauri/pdfium/libpdfium-arm64.so src-tauri/pdfium/SHA256SUMS /pdfium/
RUN cd /pdfium \
 && grep -E 'libpdfium(-arm64)?\.so$' SHA256SUMS | sed 's#src-tauri/pdfium/##' | sha256sum -c - \
 && case "${TARGETARCH:-amd64}" in \
      amd64) cp libpdfium.so /libpdfium.so ;; \
      arm64) cp libpdfium-arm64.so /libpdfium.so ;; \
      *) echo "unsupported arch ${TARGETARCH}" >&2; exit 1 ;; \
    esac

############################ 4) runtime ############################
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 1000 -s /bin/bash user \
 && mkdir -p /data /app \
 && chown -R 1000:1000 /data
COPY --from=server /llm-wiki-web /app/llm-wiki-web
COPY --from=pdfium /libpdfium.so /app/libpdfium.so
COPY --from=web /src/dist /app/dist
COPY --from=mcp /usr/local/bin/node /usr/local/bin/node
COPY --from=mcp /mcp/package.json /app/mcp-server/package.json
COPY --from=mcp /mcp/node_modules /app/mcp-server/node_modules
COPY --from=mcp /mcp/dist /app/mcp-server/dist

ENV PORT=7860 \
    BIND=0.0.0.0 \
    DATA_DIR=/data \
    DIST_DIR=/app/dist \
    RESOURCE_DIR=/app \
    PDFIUM_DYNAMIC_LIB_PATH=/app/libpdfium.so \
    RUST_BACKTRACE=1

USER 1000
WORKDIR /app
VOLUME ["/data"]
EXPOSE 7860
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD curl -fsS "http://127.0.0.1:${PORT}/api/auth/status" >/dev/null || exit 1
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/llm-wiki-web"]
