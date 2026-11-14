# The two build stages run on the build machine's own architecture and
# cross-compile when the target differs (e.g. building for the arm64 Pi on an
# amd64 PC: docker build --platform linux/arm64 .), so nothing is emulated.
FROM --platform=$BUILDPLATFORM node:22-alpine AS web
WORKDIR /web
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
ARG TARGETARCH
ARG BUILDARCH
RUN if [ "$TARGETARCH" = arm64 ] && [ "$BUILDARCH" != arm64 ]; then \
      apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
      && rm -rf /var/lib/apt/lists/* && rustup target add aarch64-unknown-linux-gnu; \
    fi
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
WORKDIR /src
COPY backend/ ./
RUN if [ "$TARGETARCH" = arm64 ] && [ "$BUILDARCH" != arm64 ]; then \
      cargo build --release --target aarch64-unknown-linux-gnu \
      && cp target/aarch64-unknown-linux-gnu/release/plex-delete-list /plex-delete-list; \
    else \
      cargo build --release && cp target/release/plex-delete-list /plex-delete-list; \
    fi

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /plex-delete-list /usr/local/bin/plex-delete-list
COPY --from=web /web/dist /app/static
ENV PDL_STATIC_DIR=/app/static \
    PDL_DB=/data/pdl.db \
    PDL_BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
CMD ["plex-delete-list"]
