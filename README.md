# AList2STRM (WebUI Edition)

专为 **AList 挂载的 302 模式网盘（如百度网盘、115、阿里云盘等）** 打造的高性能、超轻量 `.strm` 文件生成与管理工具，适合家庭 NAS 与云服务器（VPS）低内存部署。

本项目参考并派生自开源项目 [AutoFilm](https://github.com/AkimioJR/AutoFilm) 的核心 STRM 生成与路径解析算法，重构为具备现代化 WebUI、支持管理员身份验证、支持手动即时触发、极低内存开销的单二进制云原生工具。

---

## 📜 项目来源与开源声明 (Credits & License)

- **上游项目致谢**：
  - 本项目 STRM 核心算法和 BDMV 原盘解析逻辑参考/源自 [AutoFilm](https://github.com/AkimioJR/AutoFilm)（作者：[@AkimioJR](https://github.com/AkimioJR)）。
  - 非常感谢 AutoFilm 原作者为开源社区做出的贡献！
- **重构与差异说明**：
  - **专注于 STRM 核心**：完全剥离了原项目中的动漫追番（ANI RSS）和媒体库海报绘制模块，专注于解决影视资源的高效扫描与 STRM 同步。
  - **全功能 Web 控制台**：补充了原项目缺失的可视化界面，支持任务可视化管理、连接测试与实时 SSE 日志流。
  - **支持管理员身份认证**：内置安全登录拦截中间件与密码修改，确保云服务器通过域名暴露至公网时的安全性。
  - **支持手动立即触发**：支持在 Web 界面随时点击「▶ 立即运行」，摆脱只能依赖 Cron 轮询的限制。
  - **单文件交付与轻量化**：Vue 3 前端静态资产直接内嵌至单一 Rust 二进制程序中，空闲内存仅约 10MB-18MB，大幅降低云服务器部署门槛。
- **开源协议**：
  - 本项目严格遵循 **[GNU General Public License v3.0 (GPL-3.0)](./LICENSE)** 协议开源。
  - 你可以自由查看、使用、修改和分发本项目代码，但任何基于本项目的衍生分发作品均须同样以 GPL-3.0 协议开源并保留原作者与贡献者声明。
- **免责声明 (Disclaimer)**：
  - 本工具仅供个人搭建本地/私有家庭媒体库（Emby / Jellyfin / Plex）学习与资源整理之用。
  - 使用本工具产生的任何网盘请求请遵循各网盘服务商的相关用户协议，请勿将其用于非法用途或高频滥用请求。

---

## 🔐 管理员身份认证 (Security)

为了保护公网/域名访问安全，系统默认开启了管理员鉴权保护：
- **默认管理员账号**：`admin`
- **默认管理员密码**：`adminadmin`
- **修改密码**：
  - 登录后点击右上角用户名下拉菜单中的 **「🔑 修改管理员密码」**，修改后系统会自动计算加盐 SHA256 保存至 `config.yaml`。
  - 亦可直接在 `config.yaml` 文件的 `auth:` 段中修改账号与密码。

---

## 🌐 云服务器域名访问与反向代理 (Nginx / Caddy)

当部署在云服务器并通过域名（例如 `strm.yourdomain.com`）访问时，建议配置 Nginx 或 Caddy 进行 HTTPS 反代。

> [!IMPORTANT]
> **关键配置**：由于 Web 控制台集成了 **SSE 实时日志流**，反向代理时必须关闭缓冲（`proxy_buffering off;`），否则实时滚屏日志会出现延迟或卡顿。

### 1. Nginx 反代配置示例

```nginx
server {
    listen 80;
    server_name strm.yourdomain.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name strm.yourdomain.com;

    # SSL 证书配置
    ssl_certificate /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8383;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # 支持 SSE 实时日志流不被 Nginx 缓存
        proxy_buffering off;
        proxy_cache off;
        proxy_set_header Connection '';
        proxy_http_version 1.1;
        chunked_transfer_encoding off;
        proxy_read_timeout 86400s;
    }
}
```

### 2. Caddy 反代配置示例

```caddy
strm.yourdomain.com {
    reverse_proxy 127.0.0.1:8383 {
        flush_interval -1
    }
}
```

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
启动后，浏览器打开 `http://<你的服务器IP或域名>:8383` 即可进入登录页面。

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

项目内置完整的单元测试与集成测试：

```bash
cargo test
```
包含：
- 管理员认证与安全拦截流程测试（401 拦截、正确/错误密码登录、Bearer 验证、修改密码）
- AList URL 与签名生成规范测试
- BDMV 蓝光原盘折叠识别与主片选取测试
- 增量跳过与孤立文件清理测试
- AList API 连接与目录递归扫描端到端仿真测试
