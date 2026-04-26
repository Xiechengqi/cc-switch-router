# cc-switch-router

面向 `cc-switch` 的最小 Rust tunnel server。

## 技术架构

```
                  ┌──────────────────────────────────┐
                  │         cc-switch-router          │
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
- `GET /v1/public/map-points`
- `POST /v1/dashboard/presence`
- `POST /v1/auth/email/request-code`
- `POST /v1/auth/email/verify-code`
- `POST /v1/auth/session/refresh`
- `GET /v1/auth/session/me`
- `POST /v1/shares/claim-subdomain`
- `POST /v1/shares/sync`
- `POST /v1/shares/batch-sync`
- `POST /v1/share-request-logs/batch-sync`
- `POST /v1/shares/heartbeat`
- `POST /v1/shares/delete`
- `GET /`

## 二进制部署

### 准备发布包

GitHub Actions 会在 `main` 分支自动构建 Ubuntu AMD64 二进制，并更新 `latest` Release。部署时直接下载 release binary：

```bash
wget https://github.com/xiechengqi/cc-switch-router/releases/download/latest/cc-switch-router-linux-amd64 -O /usr/local/bin/cc-switch-router && chmod +x /usr/local/bin/cc-switch-router
```

### 环境变量

默认配置文件路径：`$HOME/.config/cc-switch-router/.env`

启动时如果这个文件不存在，`cc-switch-router` 会自动生成默认 `.env`，然后按该文件加载配置。进程环境变量优先级更高，会覆盖 `.env` 里的同名配置。为兼容已有部署，旧的 `PORTR_RS_*` 环境变量和 `$HOME/.config/portr-rs/.env` 仍然可读。

可用环境变量：

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CC_SWITCH_ROUTER_API_ADDR` | `0.0.0.0:8787` | HTTP 监听地址 |
| `CC_SWITCH_ROUTER_SSH_ADDR` | `0.0.0.0:2222` | SSH 监听地址 |
| `CC_SWITCH_ROUTER_TUNNEL_DOMAIN` | `0.0.0.0:8787` | 公共 tunnel 域名 |
| `CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR` | `{TUNNEL_DOMAIN}:{SSH_PORT}` | 下发给客户端的 SSH 地址（Cloudflare 代理时填源站 IP:端口） |
| `CC_SWITCH_ROUTER_USE_LOCALHOST` | `true` | 为 `false` 时 tunnel URL 使用 `https://` |
| `CC_SWITCH_ROUTER_LEASE_TTL_SECS` | `60` | Tunnel lease 有效期（秒） |
| `CC_SWITCH_ROUTER_DB_PATH` | `$HOME/.config/cc-switch-router/cc-switch-router.db` | SQLite 路径 |
| `CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS` | `300` | 清理任务执行间隔（秒） |
| `CC_SWITCH_ROUTER_LEASE_RETENTION_SECS` | `604800` | 过期 lease 保留时长（秒） |
| `CC_SWITCH_ROUTER_CLIENT_STALE_SECS` | `3600` | client 超过该时间未上报时清理其 share、lease 和 client 记录 |
| `CC_SWITCH_ROUTER_RESEND_API_KEY` | 空 | Resend API Key，用于邮箱验证码发送和 dashboard 用量读取 |
| `CC_SWITCH_ROUTER_RESEND_FROM` | 空 | 验证码邮件发件人 |
| `CC_SWITCH_ROUTER_RESEND_REPLY_TO` | 空 | 验证码邮件 Reply-To |
| `CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS` | `300` | 邮件验证码有效期（秒） |
| `CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS` | `60` | 同邮箱 / 设备发验证码冷却（秒） |
| `CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS` | `1800` | Access token 有效期（秒） |
| `CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS` | `2592000` | Refresh token 有效期（秒） |
| `CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS` | `5` | 单挑战最大输错次数 |
| `CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT` | `5` | 单邮箱每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT` | `20` | 单 IP 每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT` | `10` | 单 installation 每小时最大发送次数 |
| `CC_SWITCH_ROUTER_FREE_MODEL_IDS` | 空 | 逗号分隔的 free 模型精确 ID 列表；命中后按真实用户 IP 做并发限制 |
| `CC_SWITCH_ROUTER_FREE_MODEL_PREFIXES` | 空 | 逗号分隔的 free 模型前缀列表；用于覆盖带后缀/别名的模型名 |
| `CC_SWITCH_ROUTER_FREE_MODEL_IP_PARALLEL_LIMIT` | `1` | 单真实用户 IP 在所有 free 模型上的总并发上限；当模型列表为空时该功能关闭 |

最小生产示例：

```bash
cat > "$HOME/.config/cc-switch-router/.env" <<'EOF'
CC_SWITCH_ROUTER_API_ADDR=0.0.0.0:80
CC_SWITCH_ROUTER_SSH_ADDR=0.0.0.0:2222
CC_SWITCH_ROUTER_TUNNEL_DOMAIN=example.com
CC_SWITCH_ROUTER_USE_LOCALHOST=false
CC_SWITCH_ROUTER_RESEND_API_KEY=re_xxx
CC_SWITCH_ROUTER_RESEND_FROM=TokenSwitch <noreply@example.com>
EOF
```

### 启动

```bash
cc-switch-router
```

查看帮助：

```bash
cc-switch-router help
```

调整日志级别：

```bash
RUST_LOG=debug cc-switch-router
```

### 验证部署

```bash
curl http://127.0.0.1/v1/healthz
# {"ok":true}
```

控制台：`http://127.0.0.1/`

`/` 和 `/v1/dashboard` 默认公开可读，不需要登录。

dashboard 当前行为：

- 未登录时 share 表格中的 API key 默认脱敏
- owner 或 `shared_with_emails` 中的邮箱登录后，可看到对应 share 的 API key 明文
- 页脚 `PAGE ONLINE` 右侧在 free plan 且 Resend 返回 `x-resend-daily-quota` 时，会显示 `RESEND USAGE xx%`
- Resend 用量由服务端每 10 分钟主动请求一次并缓存；若响应头不存在，则页脚只显示 `PAGE ONLINE`

邮件登录相关端点：

- `POST /v1/auth/email/request-code` 请求邮件验证码
- `POST /v1/auth/email/verify-code` 校验验证码并签发 access / refresh token
- `POST /v1/auth/session/refresh` 刷新会话
- `GET /v1/auth/session/me` 查询当前浏览器登录态

`GET /v1/public/map-points` 返回公开地图所需的点位数据，其中 `clients` 是按坐标聚合后的地图点数组，每个点包含 `count`；`clientCount` 是符合条件的真实活跃 client 总数，两者可能不相等。

### systemd 部署示例

```ini
[Unit]
Description=cc-switch-router
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/cc-switch-router
Environment=HOME=/root
EnvironmentFile=%h/.config/cc-switch-router/.env
ExecStart=/opt/cc-switch-router/cc-switch-router
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable cc-switch-router
sudo systemctl start cc-switch-router
sudo systemctl status cc-switch-router
```

## 当前限制

- 仅实现 HTTP tunnel
- 设备私钥仍由 `cc-switch` 以本地文件方式保存，未接入系统安全存储
- 邮件验证码登录是基于服务端持久化 session 的 bearer token，不是 JWT
- Resend 用量展示依赖官方响应头 `x-resend-daily-quota`；该 header 通常只对 free plan 返回，不返回时页脚不会显示用量
- share 用量同步为"事件驱动最终一致"，由 `cc-switch` 在创建、状态变更、用量变更、删除时异步上报
- `cc-switch` 端 share 同步已做短延迟批量聚合，降低高频请求噪音
- share owner / `shared_with_emails` ACL 以 `cc-switch` 推送为准，`cc-switch-router` 负责持久化、鉴权和 dashboard 脱敏控制
- `cc-switch-router` 会定时清理超过保留期的历史 lease，以及状态为 `expired` / `deleted` 的陈旧 share 记录
- 当请求经 Cloudflare 代理进入时，free 模型限流会基于可信的 `CF-Connecting-IP` 识别真实用户 IP；直连源站时会回退到 socket peer IP，防止伪造头绕过限制
