# AList2STRM (WebUI Edition)

专为 **AList 挂载的 302 模式网盘（如百度网盘、115、阿里云盘等）** 打造的高性能、超轻量 `.strm` 文件生成与管理工具，适合家庭 NAS 与云服务器（VPS）低内存部署。

本项目参考并派生自开源项目 [AutoFilm](https://github.com/AkimioJR/AutoFilm) 的核心 STRM 生成与路径解析算法，重构为具备现代化 WebUI、支持手动即时触发、极低内存开销的单二进制云原生工具。

---

## 📜 项目来源与开源声明 (Credits & License)

- **上游项目致谢**：
  - 本项目 STRM 核心算法和 BDMV 原盘解析逻辑参考/源自 [AutoFilm](https://github.com/AkimioJR/AutoFilm)（作者：[@AkimioJR](https://github.com/AkimioJR)）。
  - 非常感谢 AutoFilm 原作者为开源社区做出的贡献！
- **重构与差异说明**：
  - **专注于 STRM 核心**：完全剥离了原项目中的动漫追番（ANI RSS）和媒体库海报绘制模块，专注于解决影视资源的高效扫描与 STRM 同步。
  - **全功能 Web 控制台**：补充了原项目缺失的可视化界面，支持任务可视化管理、连接测试与实时 SSE 日志流。
  - **支持手动立即触发**：支持在 Web 界面随时点击「▶ 立即运行」，摆脱只能依赖 Cron 轮询的限制。
  - **单文件交付与轻量化**：Vue 3 前端静态资产直接内嵌至单一 Rust 二进制程序中，空闲内存仅约 10MB-18MB，大幅降低云服务器部署门槛。
- **开源协议**：
  - 本项目严格遵循 **[GNU General Public License v3.0 (GPL-3.0)](./LICENSE)** 协议开源。
  - 你可以自由查看、使用、修改和分发本项目代码，但任何基于本项目的衍生分发作品均须同样以 GPL-3.0 协议开源并保留原作者与贡献者声明。
- **免责声明 (Disclaimer)**：
  - 本工具仅供个人搭建本地/私有家庭媒体库（Emby / Jellyfin / Plex）学习与资源整理之用。
  - 使用本工具产生的任何网盘请求请遵循各网盘服务商的相关用户协议，请勿将其用于非法用途或高频滥用请求。

---

## ✨ 核心特性

- 🎬 **专为 302 网盘（百度网盘）量身定制**：
  - 生成 `AlistURL` 格式直链（`/d/...` 并带签名 sign 与路径转义）；
  - 播放器（Emby/Jellyfin/Plex/Infuse）请求时，由 AList 动态实时 302 跳转至百度网盘最新临时直链，**播放永不失效、无需担心直链过期**。
  - 支持配置公网外部访问地址 (`public_url`)，方便内外网无缝切换。
- ⚡ **完备的 WebUI 控制台**：
  - 任务配置可视化创建、修改、一键删除；
  - **核心痛点解决**：支持随时点击 **「▶ 立即运行」** 手动触发扫描，无需傻等 Cron 周期；
  - 内置 **SSE 实时流终端**，毫秒级滚屏查看扫描进度、已处理文件数与错误详情。
- ⏳ **灵活的调度策略**：
  - 支持标准的 6 段式 Cron 定时任务（如 `0 0 3 * * *` 每天凌晨自动同步增量）。
  - 支持智能增量跳过（本地已存在的文件秒级跳过，避免重复写盘与频繁请求云盘）。
- 💿 **BDMV 蓝光原盘智能折叠**：
  - 自动识别 `/BDMV/STREAM/` 目录结构，自动挑选体积最大的正片 `.m2ts` 生成单个电影标题 `.strm`，彻底解决媒体库刮削混乱问题。
- 🚀 **极度轻量，专为云服务器优化**：
  - **单二进制自包含**：前端静态资源直接打包内嵌进二进制中，无需额外安装 Node.js、Nginx 或 Python。
  - **极低资源开销**：空闲运行时内存仅 **约 10MB - 18MB**，即便在 1C1G 的廉价 VPS 上也能流畅无感运行。
  - **超轻量 Docker**：镜像体积小于 25MB。

---

## 🛠️ 云服务器部署方式

### 方式 1：Docker Compose 部署（推荐）

创建 `docker-compose.yml`：

```yaml
version: '3.8'

services:
  alist2strm:
    image: alist2strm:latest
    build: .
    container_name: alist2strm
    restart: unless-stopped
    ports:
      - "8383:8383"            # WebUI 访问端口
    environment:
      - TZ=Asia/Shanghai
    volumes:
      - ./config:/config       # 配置文件与运行状态持久化目录
      - /mnt/media:/media      # 生成的 STRM 输出目录（挂载给 Emby / Jellyfin 扫库）
```

启动容器：
```bash
docker compose up -d
```
启动后，浏览器打开 `http://<你的服务器IP>:8383` 即可进入控制台。

---

### 方式 2：单二进制直接运行

1. 下载或编译好的二进制可执行文件：
```bash
./alist2strm --port 8383 --config ./config.yaml
```
2. 后台守护运行（如使用 systemd 或 nohup）：
```bash
nohup ./alist2strm > alist2strm.log 2>&1 &
```

---

## 📖 核心原理说明：百度网盘 302 模式

由于百度网盘的原始下载直链（RawURL）带时效签名，且通常在数小时内失效，**直接将 RawURL 写入 .strm 会导致几天后播放全部报 403**。

**本项目默认的 `AlistURL` 模式解决逻辑**：
1. 本程序调用 AList API 递归遍历目录，计算出标准的 AList 下载流路径，形如：
   `http://alist:5244/d/百度网盘/电影/星际穿越.2014.mp4?sign=...`
2. 写入本地 `.strm`；
3. 当播放器（如 Emby）开始播放该 `.strm` 时，向 AList 发起请求；
4. AList 收到请求后，由于挂载配置为 **302 模式**，AList 会**实时请求百度网盘换取最新的临时直链，并向播放器返回 302 重定向**；
5. 播放器跟随 302 直接连接百度网盘 CDN 开始流畅播放。由此实现免去服务器流量中转且链接永不失效。

---

## 🧪 自动化测试

项目内置完整的单元测试与集成测试（使用模拟 AList 服务端进行端到端全链路验证）：

```bash
cargo test
```
包含：
- AList URL 与签名生成规范测试
- BDMV 蓝光原盘折叠识别与主片选取测试
- 增量跳过与孤立文件清理测试
- AList API 连接与目录递归扫描模拟测试
