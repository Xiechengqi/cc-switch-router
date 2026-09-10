# Bark 通知运维手册

Router 原生调用 Bark Server 的 `POST /push` JSON 协议，不依赖 `/data/projects/Bark` 的 Swift 客户端代码。用户通知 Bark 与运维告警 Bark 是两条独立链路，凭据、目标、限流和健康状态均不共享。

## 用户 Bark

在“管理员 → Settings → 通知 → Bark”的用户通知分组中设置：

- `CC_SWITCH_ROUTER_BARK_SERVER_URL`：允许用户绑定的唯一 Bark Server。远端必须为 HTTPS；只有 `localhost` 或 loopback IP 可使用 HTTP。
- `CC_SWITCH_ROUTER_BARK_CREDENTIAL_MASTER_KEY`：独立的 32 字节随机密钥，以 64 位十六进制或标准 base64 表示。不要复用数据库、Binance 或其他业务密钥。
- `CC_SWITCH_ROUTER_BARK_CREDENTIAL_KEY_VERSION`：从 1 开始的版本号。
- 两个 hourly limit：分别限制单用户和全局 Bark 投递，单用户值不得超过全局值。
- 最后启用 `CC_SWITCH_ROUTER_BARK_ENABLED`。

主密钥和版本是启动期安全边界。保存后 Settings 会显示待重启；完成受控重启之前，账户页不会允许绑定。密钥或版本轮换后现有密文无法使用，用户必须重新绑定。应先保留旧数据库备份，再通知用户重绑；不要在日志或工单中粘贴主密钥、完整 Push URL 或 Device Key。

### 待重启期间的行为

只要 Settings 中保存的主密钥或版本与当前进程启动时加载的值不同，Router 就会把用户 Bark 标记为“配置待重启”，即使 `CC_SWITCH_ROUTER_BARK_ENABLED=true` 也不会混用新旧配置：

- 账户 API 返回 Bark `available=false`；新绑定、重新绑定、选中 Bark 和管理员用户渠道测试都会被拒绝。
- 尚未真正开始 HTTP 请求的 Bark `pending`、`retry`、`blocked_config` 和已保留投递会被取消，关联事件释放后按当前可用渠道重新聚合，通常回退到 Email。已经记录为 `started` 的请求允许完成，避免重复投递。
- 通知 worker 还会在发送前再次检查运行时 `enabled`/配置指纹，因此设置保存与 worker 轮询并发时也不会使用旧主密钥发送。
- Router 重启并成功加载新密钥后，Bark 才重新变为可用。若密钥或版本确实发生轮换，旧绑定会被标记为 invalid，用户需重新绑定；仅重启而配置未变化不会无故使绑定失效。

用户在“账户 → 通知设置”粘贴 Bark App 给出的完整 Push URL。Router 检查 URL 是否属于配置的 Server，持久化记录用户/IP 尝试次数，并发送验证推送。验证成功后 Device Key 才会以 XChaCha20-Poly1305 密文保存并自动选中 Bark。

`bark_binding_attempts` 只保存限流和审计所需的脱敏绑定尝试，保留 7 天；Telegram 的 `telegram_bind_tokens`（包括已使用或撤销 token）采用相同的 7 天清理期。实际用户通知写入 outbox 时会冻结完整 Bark payload，包括标题、正文、Dashboard URL、幂等 ID 和目标凭据快照；进程重启或后续重试只重放该冻结请求，不会按新配置重新渲染。

## 运维 Bark

在“Bark operator alerts”中设置独立的 Server URL、Device Key、最低严重级别并启用。此 Device Key 是热更新 secret，不会由 Settings API 回显。保存后在同页底部的渠道验证发送测试消息；该测试不会使用或影响任何用户绑定。

## 故障判定

- Bark 明确返回已知的 Device Token 查询失败（例如 `key not found` / `device token invalid`）：具体 Device Key 无效。用户投递会失效该绑定、取消旧 Bark outbox，并把事件重新排队到 Email。
- 普通 `400/404/405`：请求协议、`/push` 路由或反向代理配置错误。Bark Server 会把多种非目标错误也编码为 400，因此 Router 不会仅凭状态码删除用户绑定。
- `401/403`：Server 或反向代理授权配置错误。不会删除用户绑定。
- `408/425/429/5xx`、连接或超时：可重试故障，遵守 `Retry-After` 和 outbox 退避；服务端给出的等待时间最多接受 24 小时，零值、过去时间或更长时间都会被夹取到安全范围。
- 连续 3 次系统性失败：用户 Bark Provider 熔断 5 分钟，避免故障风暴；成功投递或成功测试会清零熔断状态。

排障时先查看“管理员 → Settings → 通知 → Bark”的 Provider 状态和最近测试，再查看 Operations 中的通知投递记录。投递列表对 Bark 固定显示 `Bark ••••`，不会泄露密文片段。确认 Server 切换后，旧 Server 的用户绑定会被标为 invalid 并自动回退 Email，这是预期的安全行为。

## 首版边界

首版使用 Router 到 Bark Server 的 TLS 与数据库中的服务端加密，不启用 Bark 客户端可选的 E2E AES 载荷加密。若使用自托管 Bark Server，应同时限制其管理面访问、保留 TLS 证书监控，并确认反向代理允许 `POST /push` JSON 请求。
