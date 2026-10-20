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
RUN cargo build --release

# 运行时阶段：轻量 Alpine 基础镜像
FROM alpine:3.20
RUN apk add --no-cache ca-certificates tzdata
WORKDIR /app

# 复制编译好的单二进制文件
COPY --from=backend-builder /app/target/release/alist2strm /app/alist2strm

# 暴露 WebUI 端口
EXPOSE 8383

# 声明持久化卷
VOLUME ["/config", "/media"]

ENTRYPOINT ["/app/alist2strm", "--config", "/config/config.yaml"]
