# portr-rs

面向 `cc-switch` 的最小 Rust tunnel server。

## 技术架构

```
                  ┌──────────────────────────────────┐
                  │            portr-rs               │
                  │                                   │
  HTTPS ──────►  │  HTTP API + Subdomain Proxy (:80) │
  (Cloudflare)   │                                   │
                  │  SSH Reverse Forwarding  (:2222)  │
  SSH ─────────► │                                   │
                  │  SQLite (lease/share/install)      │
                  └──────────────────────────────────┘
```

单进程同时承载三个职责：

- **HTTP 服务** — API 端点 + 基于 Host subdomain 的反向代理，共用同一端口
- **SSH 服务** — 基于 `russh` 的 reverse forwarding，一次性密码认证
- **数据存储** — SQLite，存储 installation、lease、share 等状态

核心依赖：`axum`、`russh`、`rusqlite`、`tokio`

当前实现的端点：

- `POST /v1/installations/register`
- `POST /v1/tunnels/lease`
- `GET /v1/healthz`
- `GET /v1/dashboard`
- `POST /v1/shares/sync`
- `POST /v1/shares/batch-sync`
- `POST /v1/shares/delete`
- `GET /admin`

## 二进制部署

### 准备发布包

GitHub Actions 会在 `main` 分支自动构建 Ubuntu AMD64 二进制，并更新 `latest` Release。部署时直接下载 release binary：

```bash
wget https://github.com/xiechengqi/portr-rs/releases/download/latest/portr-rs-linux-amd64 -O /usr/local/bin/portr-rs && chmod +x /usr/local/bin/portr-rs
```

### 环境变量

默认配置文件路径：`$HOME/.config/portr-rs/.env`

启动时如果这个文件不存在，`portr-rs` 会自动生成默认 `.env`，然后按该文件加载配置。进程环境变量优先级更高，会覆盖 `.env` 里的同名配置。

可用环境变量：

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PORTR_RS_API_ADDR` | `0.0.0.0:8787` | HTTP 监听地址 |
| `PORTR_RS_SSH_ADDR` | `0.0.0.0:2222` | SSH 监听地址 |
| `PORTR_RS_TUNNEL_DOMAIN` | `0.0.0.0:8787` | 公共 tunnel 域名 |
| `PORTR_RS_SSH_PUBLIC_ADDR` | `{TUNNEL_DOMAIN}:{SSH_PORT}` | 下发给客户端的 SSH 地址（Cloudflare 代理时填源站 IP:端口） |
| `PORTR_RS_USE_LOCALHOST` | `true` | 为 `false` 时 tunnel URL 使用 `https://` |
| `PORTR_RS_LEASE_TTL_SECS` | `60` | Tunnel lease 有效期（秒） |
| `PORTR_RS_DB_PATH` | `$HOME/.config/portr-rs/portr-rs.db` | SQLite 路径 |
| `PORTR_RS_CLEANUP_INTERVAL_SECS` | `300` | 清理任务执行间隔（秒） |
| `PORTR_RS_LEASE_RETENTION_SECS` | `604800` | 过期 lease 保留时长（秒） |

最小生产示例：

```bash
cat > "$HOME/.config/portr-rs/.env" <<'EOF'
PORTR_RS_API_ADDR=0.0.0.0:80
PORTR_RS_SSH_ADDR=0.0.0.0:2222
PORTR_RS_TUNNEL_DOMAIN=example.com
PORTR_RS_USE_LOCALHOST=false
EOF
```

### 启动

```bash
portr-rs
```

查看帮助：

```bash
portr-rs help
```

调整日志级别：

```bash
RUST_LOG=debug portr-rs
```

### 验证部署

```bash
curl http://127.0.0.1/v1/healthz
# {"ok":true}
```

控制台：`http://127.0.0.1/admin`

`/admin` 和 `/v1/dashboard` 默认公开可读，不需要登录。

### systemd 部署示例

```ini
[Unit]
Description=portr-rs
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/portr-rs
Environment=HOME=/root
EnvironmentFile=%h/.config/portr-rs/.env
ExecStart=/opt/portr-rs/portr-rs
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable portr-rs
sudo systemctl start portr-rs
sudo systemctl status portr-rs
```

## 当前限制

- 仅实现 HTTP tunnel
- 设备私钥仍由 `cc-switch` 以本地文件方式保存，未接入系统安全存储
- share 用量同步为"事件驱动最终一致"，由 `cc-switch` 在创建、状态变更、用量变更、删除时异步上报
- `cc-switch` 端 share 同步已做短延迟批量聚合，降低高频请求噪音
- `portr-rs` 会定时清理超过保留期的历史 lease，以及状态为 `expired` / `deleted` 的陈旧 share 记录
