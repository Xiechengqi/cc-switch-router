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

Client Web tunnel 的边界策略：静态资源和明确列出的登录/OAuth 回调公开；其余 `/web-api/*` 默认要求 owner/admin 身份，Router 鉴权后向 client 注入可信身份头。`/api/*`、`/v1/*`、`/_ctl/*` 和 `/_share-router/*` 不通过 client web tunnel 暴露。流式管理接口必须使用 `Authorization` header，不接受 query-string token。

核心依赖：`axum`、`russh`、`rusqlite`、`tokio`

当前实现的端点：

- `POST /v1/installations/register`
- `POST /v1/installations/heartbeat`
- `POST /v1/installations/setup-completed`
- `POST /v1/tunnels/lease`
- `POST /v1/tunnels/lease/renew`
- `GET /v1/healthz`
- `GET /v1/dashboard`
- `GET /v1/public/map-points`
- `GET /v1/public/installations/:installation_id/payout-profile`
- `GET /v1/public/payout-profiles?installationIds=...`
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
- `POST /v1/shares/prune`
- `GET /v1/chat/clients/:installation_id/room`
- `GET /v1/chat/rooms/:room_id/messages`
- `POST /v1/chat/rooms/:room_id/messages`（需要登录）
- `GET /v1/chat/rooms`（当前登录用户曾打开的聊天室）
- `GET /`

## 二进制部署

### 准备发布包

GitHub Actions 会在 `main` 分支自动构建 Ubuntu AMD64 二进制，并更新 `latest` Release。部署时直接下载 release binary：

```bash
wget https://github.com/xiechengqi/cc-switch-router/releases/download/latest/cc-switch-router-linux-amd64 -O /usr/local/bin/cc-switch-router && chmod +x /usr/local/bin/cc-switch-router
```

### 环境变量

默认配置文件路径：`$HOME/.cc-switch-router/.env`

启动时如果这个文件不存在，`cc-switch-router` 会自动生成默认 `.env`，然后按该文件加载配置。进程环境变量优先级更高，会覆盖 `.env` 里的同名配置。

可用环境变量：

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CC_SWITCH_ROUTER_API_ADDR` | `0.0.0.0:80` | HTTP 监听地址 |
| `CC_SWITCH_ROUTER_SSH_ADDR` | `0.0.0.0:2222` | SSH 监听地址 |
| `CC_SWITCH_ROUTER_TUNNEL_DOMAIN` | `0.0.0.0:8787` | 公共 tunnel 域名 |
| `CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR` | `{TUNNEL_DOMAIN}:{SSH_PORT}` | 下发给客户端的 SSH 地址（Cloudflare 代理时填源站 IP:端口） |
| `CC_SWITCH_ROUTER_USE_LOCALHOST` | `false` | 为 `false` 时 tunnel URL 使用 `https://` |
| `CC_SWITCH_ROUTER_LEASE_TTL_SECS` | `60` | Tunnel lease 有效期（秒）；已连接 client 使用签名续期 API 原连接续期，不按该周期重建 SSH |
| `CC_SWITCH_ROUTER_DB_PATH` | `$HOME/.cc-switch-router/cc-switch-router.db` | SQLite 路径 |
| `CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS` | `300` | 清理任务执行间隔（秒） |
| `CC_SWITCH_ROUTER_LEASE_RETENTION_SECS` | `86400` | 过期 lease 保留时长（秒） |
| `CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS` | `30` | Share 请求记录和图片请求历史保留天数，范围 1-365；不影响累计 Token 用量 |
| `CC_SWITCH_ROUTER_CLIENT_STALE_SECS` | `3600` | client 超过该时间未心跳时标记离线，并清理其 share、lease 与内存路由 |
| `CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS` | `21600` | 离线 client 的 installation 记录（含 payout）保留时长，超时后删除；必须 >= `CLIENT_STALE_SECS` |
| `CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE` | `60` | 单可信来源每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST` | `20` | 单可信来源允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE` | `600` | Router 全局每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST` | `200` | Router 全局允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE` | `10` | 单公钥每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST` | `3` | 单公钥允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS` | `600` | 来源/公钥尝试计数器的空闲释放时间（秒） |
| `CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS` | `8192` | 内存中同时保留的来源尝试计数器上限 |
| `CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS` | `16384` | 内存中同时保留的公钥尝试计数器上限 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT` | `30` | 单来源 10 分钟内持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT` | `100` | 单来源每小时持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT` | `300` | 单来源每日持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT` | `300` | Router 全局 10 分钟内持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT` | `1000` | Router 全局每小时持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT` | `5000` | Router 全局每日持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK` | `50000` | 未绑定 Owner 的 installation 记录达到该水位后暂停新身份准入 |
| `CC_SWITCH_ROUTER_RESEND_API_KEY` | 空 | Resend API Key，用于验证码、Client 生命周期/聊天室邮件和 dashboard 用量读取；未配置时禁止发送聊天消息 |
| `CC_SWITCH_ROUTER_RESEND_FROM` | 空 | 邮件发件人，可填裸邮箱或 `TokenSwitch <noreply@example.com>`；裸邮箱会自动显示为 `TokenSwitch <邮箱>` |
| `CC_SWITCH_ROUTER_RESEND_FROM_NAME` | `TokenSwitch` | `CC_SWITCH_ROUTER_RESEND_FROM` 为裸邮箱时使用的发件人显示名 |
| `CC_SWITCH_ROUTER_RESEND_REPLY_TO` | 空 | 验证码、Client 生命周期与聊天室邮件的 Reply-To |
| `CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED` | `true` | Client 注册/离线邮件总开关；通知仅发送至对应 Client 当前已验证的 Owner 邮箱 |
| `CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS` | `180` | 连续缺少可信签名心跳多久后确认离线；安全下限为 180 秒 |
| `CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS` | `120` | 离线 Client 恢复后持续稳定多久才结束原离线 episode |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS` | `1800` | 同一 Client 两次离线通知的最短间隔 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS` | `60` | 同一收件人的离线事件合并窗口；可信注册固定使用 5 秒 debounce |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS` | `300` | 注册或离线通知风暴检测窗口 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS` | `5` | 进入 incident digest 的绝对 Client 数阈值 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT` | `20` | 进入 incident digest 的受监控 Client 百分比阈值 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS` | `1800` | 同一 incident digest 的最短更新间隔 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT` | `10` | Offline lane 单收件人每小时发送硬上限 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT` | `50` | Offline lane 的 Router 全局每小时发送硬上限 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT` | `3` | Registration lane 单收件人每小时发送硬上限 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT` | `10` | Registration lane 的 Router 全局每小时发送硬上限 |
| `CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS` | `300` | 邮件验证码有效期（秒） |
| `CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS` | `60` | 同邮箱 / 设备发验证码冷却（秒） |
| `CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS` | `1800` | Access token 有效期（秒） |
| `CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS` | `2592000` | Refresh token 有效期（秒） |
| `CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS` | `5` | 单挑战最大输错次数 |
| `CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT` | `30` | 单邮箱每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT` | `20` | 单 IP 每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT` | `10` | 单 installation 每小时最大发送次数 |
| `CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT` | `1` | 所有 `for_sale = Free` share 共用的单真实用户 IP 并发上限；设为 `0` 可关闭 |

注册准入先使用内存中的来源、全局和公钥尝试计数器削平瞬时流量，再对真正创建的新 installation 身份执行 SQLite 持久化的来源/全局 10 分钟、小时和每日额度。进程重启会重置内存尝试计数器，但不会重置持久化的新身份额度。达到任一限制时接口返回 HTTP `429` 并携带 `Retry-After`；使用已有公钥恢复已注册 installation 仍受尝试速率保护，但不消耗新身份额度，也不受未绑定 installation 水位线阻断。

最小生产示例：

```bash
cat > "$HOME/.cc-switch-router/.env" <<'EOF'
CC_SWITCH_ROUTER_API_ADDR=0.0.0.0:80
CC_SWITCH_ROUTER_SSH_ADDR=0.0.0.0:2222
CC_SWITCH_ROUTER_TUNNEL_DOMAIN=example.com
CC_SWITCH_ROUTER_USE_LOCALHOST=false
CC_SWITCH_ROUTER_RESEND_API_KEY=re_xxx
CC_SWITCH_ROUTER_RESEND_FROM=noreply@example.com
EOF
```

Client 生命周期通知使用持久化 outbox、固定 Resend 幂等键和离线 episode 去重，注册与离线邮件都只发送至对应 Client 当前已验证的 Owner 邮箱。关闭总开关时，Router 会推进在线状态 baseline 并抑制待发记录；以后重新启用不会补发停用期间的历史通知。多 Client 在窗口内集中注册或离线时会按 Owner 合并为 digest。Offline lane 使用独立的单收件人/全局 `10/50` 小时额度，registration lane 使用独立的 `3/10` 小时额度，两者互不占用。未完成的 outbox 会持续保留，已发送、dead-letter、取消和抑制记录保留 30 天供审计。

Client 聊天室与 `installation.id` 一一对应，只为已验证 Owner 的 Client 建立。历史消息公开可读，发送消息必须使用 Router 登录 Session；普通用户 API Token 不能发送。匿名访客的最近聊天室和已读游标只保存在当前浏览器，登录后会一次性合并到服务端用户记录。非 Owner 消息在同一聊天室内从第一条消息开始使用固定 60 秒窗口聚合，窗口内每条消息都完整写入同一封 Owner 邮件；Owner 自己的消息不会触发邮件。消息与邮件事件在同一 SQLite 事务落库，后台使用固定 Resend 幂等键、claim lease、重试和 dead-letter。Client 被清理后聊天室转为公开只读归档并保留 60 天，同一 Client 在期限内恢复时沿用原房间。

旧 `/v1/board/*` 数据不迁移也不删除；GET 在一个兼容版本内保持只读，POST/置顶/精选/删除均返回 HTTP `410 Gone`。旧 `CC_SWITCH_ROUTER_BOARD_*` 和 Board Telegram 开关仅作为兼容配置保留，不影响 Client 聊天室。

Setup 完成通知采用 Router-first 发布顺序：先部署支持 `POST /v1/installations/setup-completed` 的 Router，再升级 Server。新 Server 在 setup 成功后显式提交签名完成事件；尚未升级的旧 Server 首次 claim Client tunnel 时，Router 会创建临时 fallback 并等待固定 30 分钟，宽限期内若收到显式事件就由显式事件接管，否则才发送 legacy 注册通知。fallback 仅覆盖刚注册并很快 claim tunnel 的 Client，旧 installation 重连不会触发。所有受支持 Server 版本都已实现显式上报且最旧版本退出后，应删除该兼容桥。

升级兼容：`CC_SWITCH_ROUTER_CLIENT_ALERT_EMAILS` 与 `CC_SWITCH_ROUTER_CLIENT_OFFLINE_NOTIFY_OWNER` 已废弃并被忽略，即使旧 `.env` 仍保留这些键也不会生效。Owner-only 通知仅由 `CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED` 总开关控制；旧部署若保持该开关为 `true`，升级后收件人会切换为对应 Client 当前已验证的 Owner。

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

- Client installation 可选携带公开收款资料（一个 EVM 地址、USDC/USDT、BSC/Base/Arbitrum One 多选网络）；Client 卡片显示摘要，详情抽屉显示完整地址和自行声明状态。
- 即使 installation 暂无 share/client tunnel，只要配置了收款资料，也会出现在 Client 列表。

- 未登录时 share 表格中的 API key 默认脱敏
- owner 或 `shared_with_emails` 中的邮箱登录后，可看到对应 share 的 API key 明文
- 页脚 `PAGE ONLINE` 右侧在 free plan 且 Resend 返回 `x-resend-daily-quota` 时，会显示 `RESEND USAGE xx%`
- Resend 用量由服务端每 10 分钟主动请求一次并缓存；若响应头不存在，则页脚只显示 `PAGE ONLINE`

邮件登录相关端点：

- `POST /v1/auth/email/request-code` 请求邮件验证码
- `POST /v1/auth/email/verify-code` 校验验证码并签发 access / refresh token
- `POST /v1/auth/session/refresh` 刷新会话
- `GET /v1/auth/session/me` 查询当前浏览器登录态

`GET /v1/public/map-points` 返回公开地图所需的点位数据，其中 `clients` 是按国家质心聚合后的地图点数组，每个点包含 `count`；`clientCount` 是符合条件的真实活跃 client 总数，两者可能不相等。

### systemd 部署示例

```ini
[Unit]
Description=cc-switch-router
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/cc-switch-router
Environment=HOME=/root
EnvironmentFile=%h/.cc-switch-router/.env
ExecStart=/opt/cc-switch-router/cc-switch-router
Restart=always
RestartSec=3
KillSignal=SIGTERM
TimeoutStopSec=45
StandardOutput=append:/var/log/cc-switch-router.log
StandardError=inherit

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable cc-switch-router
sudo systemctl start cc-switch-router
sudo systemctl status cc-switch-router
```

Router 收到 `SIGTERM` 后先停止 HTTP 接入并最多排空 30 秒，再关闭 SSH
listener。日志使用 append 模式；生产环境应由 `logrotate` 或 journald 负责轮转，
不要在重启脚本中截断日志。

## 当前限制

- 仅实现 HTTP tunnel
- 设备私钥仍由 `cc-switch` 以本地文件方式保存，未接入系统安全存储
- 邮件验证码登录是基于服务端持久化 session 的 bearer token，不是 JWT
- Resend 用量展示依赖官方响应头 `x-resend-daily-quota`；该 header 通常只对 free plan 返回，不返回时页脚不会显示用量
- share 用量同步为"事件驱动最终一致"，由 `cc-switch` 在创建、状态变更、用量变更、删除时异步上报
- `cc-switch` 端 share 同步已做短延迟批量聚合，降低高频请求噪音
- share owner / `shared_with_emails` ACL 以 `cc-switch` 推送为准，`cc-switch-router` 负责持久化、鉴权和 dashboard 脱敏控制
- 收款资料以 installation 为作用域，通过 Client Ed25519 签名的 `PUT /v1/installations/payout-profile` 同步；清除会保留 revision tombstone，防止旧请求恢复地址。资料公开可读，但 `self_declared` 不代表 Router 已验证钱包所有权。
- `cc-switch-router` 会定时清理超过保留期的历史 lease，以及状态为 `expired` / `deleted` 的陈旧 share 记录
- 当请求经 Cloudflare 代理进入时，free share 限流会基于可信的 `CF-Connecting-IP` 识别真实用户 IP；直连源站时会回退到 socket peer IP，防止伪造头绕过限制
