# syntax=docker/dockerfile:1
# 多阶段构建 Dockerfile，生成超轻量 Docker 镜像 (< 25MB)
FROM node:20-alpine AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package.json ./
RUN npm install
COPY frontend ./
RUN npm run build

FROM rust:alpine AS backend-builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=frontend-builder /app/frontend/dist ./frontend/dist
# Cargo 的 registry 与 target 用 BuildKit cache mount：
# - 不进入镜像层，因此 COPY dist 导致本层缓存失效时，已编译的依赖仍然保留；
# - 之后只重编 alist2strm 这一个 crate，而不是全部依赖（约 8 分钟 -> 1~2 分钟）。
# touch src/web.rs：dist 是通过 rust-embed 在编译期嵌入的，强制重编 crate 确保新前端被嵌入。
# cache mount 不会提交进镜像层，所以必须在同一个 RUN 里把二进制复制到 /app 下。
RUN --mount=type=cache,id=alist2strm-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=alist2strm-cargo-target,target=/app/target \
    touch src/web.rs && cargo build --release \
 && cp target/release/alist2strm /app/alist2strm

# 运行时阶段：轻量 Alpine 基础镜像
FROM alpine:3.20
RUN apk add --no-cache ca-certificates tzdata
WORKDIR /app

# 复制编译好的单二进制文件
COPY --from=backend-builder /app/alist2strm /app/alist2strm

# 暴露 WebUI 端口
EXPOSE 8383

# 声明持久化卷
VOLUME ["/config", "/media"]

ENTRYPOINT ["/app/alist2strm", "--config", "/config/config.yaml"]
