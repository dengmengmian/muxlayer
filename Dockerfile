# Build stage — headless only, 不带 desktop feature 就不链接 tauri / webkit
FROM rust:1-bookworm AS builder

WORKDIR /build

# 只需 openssl(reqwest)+ pkg-config;不再需要 webkit/gtk/appindicator
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY src-tauri/ ./

# --no-default-features 去掉 desktop(tauri/webkit),只编 headless 网关
RUN cargo build --release --no-default-features --features cli --bin agentgate-serve

# Runtime stage — slim,无 GUI 库,非 root
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/agentgate-serve /usr/local/bin/agentgate-serve

# 非 root 运行;数据目录归该用户所有(token + DB 都落在这里,随 volume 持久化)
RUN useradd --system --uid 10001 --home-dir /data agentgate \
    && mkdir -p /data \
    && chown -R agentgate:agentgate /data

ENV AGENTGATE_DB_PATH=/data
ENV AGENTGATE_HOST=0.0.0.0
ENV AGENTGATE_PORT=9090

USER agentgate
EXPOSE 9090

# 编排器探活:打 /health。端口解析顺序与 agentgate-serve 一致:MUXLAYER_PORT 优先,
# 其次 AGENTGATE_PORT,默认 9090。
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${MUXLAYER_PORT:-${AGENTGATE_PORT:-9090}}/health" || exit 1

ENTRYPOINT ["agentgate-serve"]
