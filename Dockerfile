FROM node:20-bookworm-slim AS frontend-build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY index.html tsconfig.json tsconfig.node.json vite.config.ts ./
COPY public ./public
COPY src ./src
RUN npm run build

FROM rust:1.85-bookworm AS backend-build
WORKDIR /app/src-server
COPY src-server/Cargo.toml src-server/Cargo.lock ./
COPY src-server/src ./src
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates gdal-bin && \
    rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --shell /usr/sbin/nologin appuser
WORKDIR /app
COPY --from=backend-build /app/src-server/target/release/dtm-server /usr/local/bin/dtm-server
COPY --from=frontend-build /app/dist /app/dist
RUN mkdir -p /var/cache/ontario-dtm-download && \
    chown -R appuser:appuser /app /var/cache/ontario-dtm-download
ENV FRONTEND_DIST=/app/dist
ENV DTM_CACHE_DIR=/var/cache/ontario-dtm-download
EXPOSE 3000
VOLUME ["/var/cache/ontario-dtm-download"]
USER appuser
CMD ["dtm-server"]
