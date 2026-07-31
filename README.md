# cc-switch-router

TokenSwitch 的公共汇聚层。为 `cc-switch-server` 实例提供公网子域名与反向隧道,并在其上承载额度共享市场、主机供给市场与多区域 Router 联邦。

| Region | 24h usage |
| --- | --- |
| [japan](https://jptokenswitch.cc) | ![japan 24h](https://jptokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |
| [singapore](https://sgptokenswitch.cc) | ![singapore 24h](https://sgptokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |
| [hongkong](https://hktokenswitch.cc) | ![hongkong 24h](https://hktokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |
| [usa](https://ustokenswitch.cc) | ![usa 24h](https://ustokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |

- 架构与实现现状 → [ARCHITECTURE.md](ARCHITECTURE.md)
- 与客户端的接口契约 → [PROTOCOL.md](PROTOCOL.md)
- 手动 UI 回归清单 → [UI_TEST_PLAN.md](UI_TEST_PLAN.md)
- 历史改名与迁移 → [MIGRATION.md](MIGRATION.md)

## 技术架构

```
                  ┌────────────────────────────────────┐
                  │          cc-switch-router          │
  HTTPS ────────► │  HTTP API + 子域名反代 + 内嵌前端 (:80) │
  (Cloudflare)    │                                    │
  SSH   ────────► │  SSH 反向隧道服务端 (:2222)           │
                  │                                    │
                  │  SQLite ×2(主库 + 独立 metrics 库)   │
                  └────────────────────────────────────┘
                                  ▲
                                  │ SSH reverse tunnel
                                  │
                         cc-switch-server 实例
```

单进程同时承载三个职责:

- **HTTP 服务** — API 端点 + 基于 Host subdomain 的反向代理 + 内嵌前端,共用同一端口
- **SSH 服务** — 基于 `russh` 的 reverse forwarding,一次性密码认证
- **数据存储** — SQLite,存储 installation、lease、share、市场与通知等状态

核心依赖:`axum`、`russh`、`rusqlite`、`tokio`、`reqwest`

## 客户端

**`cc-switch-server` 是本 Router 的唯一客户端。** 它是无桌面依赖的 Rust server,自身为 Claude Code / Codex CLI / Gemini CLI 提供本地反代,并通过本 Router 获得公网可达性。

早期的 `cc-switch` Tauri 桌面版已不再作为客户端,相关兼容代码已移除,详见 [MIGRATION.md](MIGRATION.md)。

远程主机上的部署由仓库内 `install-client.sh` 负责,它会下载 `cc-switch-server` 二进制并完成初始化。Client Market 的主机开通流程会自动调用该脚本。

客户端与 Router 之间的注册、lease、建链、控制平面与身份注入契约,见 [PROTOCOL.md](PROTOCOL.md)。

## 边界策略

Client Web tunnel:静态资源和明确列出的登录/OAuth 回调公开;其余 `/web-api/*` 默认要求 owner/admin 身份,Router 鉴权后向 client 注入可信身份头。`/api/*`、`/v1/*`、`/_ctl/*` 和 `/_share-router/*` 不通过 client web tunnel 暴露。流式管理接口必须使用 `Authorization` header,不接受 query-string token。

## API 端点

API 路由按域分组概览如下,协议细节见 [PROTOCOL.md](PROTOCOL.md)。

| 域 | 路径数 | 认证方式 | 代表端点 |
|---|---:|---|---|
| `/v1/client-market/*` | 28 | 用户 Session | `hosts`、`quotes`、`quotes/:id/commit`、`providers`、`my-rentals`、`terminal/ws` |
| `/v1/admin/*` | 约 36 | Session + admin 判定 | `settings/values`、`version`、`upgrade`、`metrics/*`、`audit`、`logs/router/tail`、`market-billing/disputes` |
| `/v1/shares/*` | 17 | installation bearer / Ed25519 签名 | `claim-subdomain`、`sync`、`batch-sync`、`descriptor-batch-sync`、`pending-edits`、`edit-ack`、`edit-events`、`runtime-refresh`、`heartbeat`、`prune` |
| `/v1/installations/*` | 11 | Ed25519 签名 / bearer | `register`、`heartbeat`、`setup-completed`、`report-status`、`client-tunnel`、`client-tunnel/claim`、`bind-owner-email` |
| `/v1/chat/*` | 9 | 公开读 / Session 写 | `clients/:installation_id/room`、`rooms/:room_id/messages`、`rooms/:room_id/stream`；不存在 Share 独立房间 |
| `/v1/market/*`、`/v1/markets/*` | 11 | 公开读 / 用户 Session / 市场 bearer token | `shares`、`shares/headroom`、`request-logs/batch`、`share-states`、`tunnel/lease` |
| `/v1/share-market/*` | 9 | 公开 catalog / 用户 Session | `listings`、`owned-shares`、`seats/:id/rent`、`subscriptions/:id/release`、`force-revoke`；停止挂售后无活跃租约可再次 `POST listings` |
| `/v1/market-access/*` | 6 | 用户 Session / scoped API Token | `dashboard`、`policies/:product_kind`、`counterparties`、买家授信、黑名单模式公共额度 |
| `/v1/market-billing/*` | 10 | 用户 Session | `dashboard`、`supplier-profiles`、`accounts/:id/settle`、`request-settlement`、`accounts/:id/invoices`、付款声明/确认/拒绝与争议 |
| `/v1/gateway/*`、`/v1/gateways/*` | 5 | HMAC 签名(`x-cc-gateway-*`) | `register`、`shares`、`shares/feedback`、`request-logs/batch` |
| `/v1/auth/*` | 5 | 公开 / Session | `email/request-code`、`email/verify-code`、`session/refresh`、`session/me`、`session/logout` |
| `/v1/tunnels/*` | 4 | Ed25519 签名 | `lease`、`lease/renew`、`activate`、`state` |
| `/v1/account/*` | 2 | Session | `payment-profile`、`payment-assets/:id` |
| `/v1/public/*` | 4 | 公开 | `map-points`、`network-stats`、`embed/global.svg`、`embed/usage/:username` |
| `/share-api/*` | 4 | 子域名上下文,Session 可选 | `context`、`share`、`auth/me`、`share/settings` |
| `/v1/dashboard/*`、`/v1/me/*` | 10 | Session | `dashboard`、`presence`、`ux-events`、`me/api-token`、`me/shares`、`me/profile`、`me/usage/consumer`、`me/usage/provider` |
| `/v1/board/*` | 5 | — | 遗留接口,写操作返回 `410 Gone` |
| 其余单例 | 约 15 | 混合 | `healthz`、`regions`、`announcement`、`map-display`、`client-tunnel/subdomain-availability`、`_market/proxy/*`、`_gateway/proxy/*`、`*path`(前端与反代 catch-all) |

## 二进制部署

### 准备发布包

GitHub Actions 会在 `master` 分支自动构建 Ubuntu AMD64 二进制,并更新 `latest` Release。部署时直接下载 release binary:

```bash
wget https://github.com/xiechengqi/cc-switch-router/releases/download/latest/cc-switch-router-linux-amd64 -O /usr/local/bin/cc-switch-router && chmod +x /usr/local/bin/cc-switch-router
```

前端资源在编译期由 `build.rs` 内嵌进二进制,`cargo build --release` 前必须先执行 `(cd frontend && npm ci && npm run build)`。

### 环境变量

默认配置文件路径:`$HOME/.cc-switch-router/.env`

启动时如果这个文件不存在,`cc-switch-router` 会自动生成默认 `.env`,然后按该文件加载配置。进程环境变量优先级更高,会覆盖 `.env` 里的同名配置。

可用环境变量:

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CC_SWITCH_ROUTER_API_ADDR` | `0.0.0.0:80` | HTTP 监听地址 |
| `CC_SWITCH_ROUTER_SSH_ADDR` | `0.0.0.0:2222` | SSH 监听地址 |
| `CC_SWITCH_ROUTER_TUNNEL_DOMAIN` | `0.0.0.0:8787` | 公共 tunnel 域名 |
| `CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR` | `{TUNNEL_DOMAIN}:{SSH_PORT}` | 下发给客户端的 SSH 地址(Cloudflare 代理时填源站 IP:端口) |
| `CC_SWITCH_ROUTER_OWNER_EMAIL` | `router@{TUNNEL_DOMAIN}` | Client Market 默认选中的官方 Host Provider 邮箱 |
| `CC_SWITCH_ROUTER_USE_LOCALHOST` | `false` | 为 `false` 时 tunnel URL 使用 `https://` |
| `CC_SWITCH_ROUTER_LEASE_TTL_SECS` | `60` | Tunnel lease 有效期(秒);已连接 client 使用签名续期 API 原连接续期,不按该周期重建 SSH |
| `CC_SWITCH_ROUTER_DB_PATH` | `$HOME/.cc-switch-router/cc-switch-router.db` | SQLite 路径 |
| `CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS` | `300` | 清理任务执行间隔(秒) |
| `CC_SWITCH_ROUTER_LEASE_RETENTION_SECS` | `86400` | 过期 lease 保留时长(秒) |
| `CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS` | `30` | Share 请求记录和图片请求历史保留天数,范围 1-365;不影响累计 Token 用量 |
| `CC_SWITCH_ROUTER_CLIENT_STALE_SECS` | `3600` | client 超过该时间未心跳时标记离线,并清理其 share、lease 与内存路由 |
| `CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS` | `21600` | 离线 client 的 installation 记录保留时长,超时后删除;必须 >= `CLIENT_STALE_SECS` |
| `CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE` | `60` | 单可信来源每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST` | `20` | 单可信来源允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE` | `600` | Router 全局每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST` | `200` | Router 全局允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE` | `10` | 单公钥每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST` | `3` | 单公钥允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS` | `600` | 来源/公钥尝试计数器的空闲释放时间(秒) |
| `CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS` | `8192` | 内存中同时保留的来源尝试计数器上限 |
| `CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS` | `16384` | 内存中同时保留的公钥尝试计数器上限 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT` | `30` | 单来源 10 分钟内持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT` | `100` | 单来源每小时持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT` | `300` | 单来源每日持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT` | `300` | Router 全局 10 分钟内持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT` | `1000` | Router 全局每小时持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT` | `5000` | Router 全局每日持久化新 installation 身份额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK` | `50000` | 未绑定 Owner 的 installation 记录达到该水位后暂停新身份准入 |
| `CC_SWITCH_ROUTER_RESEND_API_KEY` | 空 | Resend API Key,用于验证码、Client 生命周期/聊天室邮件和 dashboard 用量读取;未配置时禁止发送聊天消息 |
| `CC_SWITCH_ROUTER_RESEND_FROM` | 空 | 邮件发件人,可填裸邮箱或 `TokenSwitch <noreply@example.com>`;裸邮箱会自动显示为 `TokenSwitch <邮箱>` |
| `CC_SWITCH_ROUTER_RESEND_FROM_NAME` | `TokenSwitch` | `CC_SWITCH_ROUTER_RESEND_FROM` 为裸邮箱时使用的发件人显示名 |
| `CC_SWITCH_ROUTER_RESEND_REPLY_TO` | 空 | 验证码、Client 生命周期与聊天室邮件的 Reply-To |
| `CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED` | `true` | Client 注册/离线邮件总开关;通知仅发送至对应 Client 当前已验证的 Owner 邮箱 |
| `CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS` | `180` | 连续缺少可信签名心跳多久后确认离线;安全下限为 180 秒 |
| `CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS` | `120` | 离线 Client 恢复后持续稳定多久才结束原离线 episode |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS` | `1800` | 同一 Client 两次离线通知的最短间隔 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS` | `60` | 同一收件人的离线事件合并窗口;可信注册固定使用 5 秒 debounce |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS` | `300` | 注册或离线通知风暴检测窗口 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS` | `5` | 进入 incident digest 的绝对 Client 数阈值 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT` | `20` | 进入 incident digest 的受监控 Client 百分比阈值 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS` | `1800` | 同一 incident digest 的最短更新间隔 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT` | `10` | Offline lane 单收件人每小时发送硬上限 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT` | `50` | Offline lane 的 Router 全局每小时发送硬上限 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT` | `3` | Registration lane 单收件人每小时发送硬上限 |
| `CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT` | `10` | Registration lane 的 Router 全局每小时发送硬上限 |
| `CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS` | `300` | 邮件验证码有效期(秒) |
| `CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS` | `60` | 同邮箱 / 设备发验证码冷却(秒) |
| `CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS` | `1800` | Access token 有效期(秒) |
| `CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS` | `2592000` | Refresh token 有效期(秒) |
| `CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS` | `5` | 单挑战最大输错次数 |
| `CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT` | `30` | 单邮箱每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT` | `20` | 单 IP 每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT` | `10` | 单 installation 每小时最大发送次数 |
| `CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT` | `1` | 所有 `for_sale = Free` share 共用的单真实用户 IP 并发上限;设为 `0` 可关闭 |
| `CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS` | 内置三个 `http://` 源站 | Client Market 主机 IP 情报服务,逗号分隔的 base URL,按顺序尝试。**每台登记主机的 IP 都会发送到这些端点**,应由 Router 运维方自建或交给可信任全量主机清单的一方。缺少 scheme 时按 `https://` 处理;仍使用 `http://` 时启动会打印告警。结果缓存 6 小时 |

注册准入先使用内存中的来源、全局和公钥尝试计数器削平瞬时流量,再对真正创建的新 installation 身份执行 SQLite 持久化的来源/全局 10 分钟、小时和每日额度。进程重启会重置内存尝试计数器,但不会重置持久化的新身份额度。达到任一限制时接口返回 HTTP `429` 并携带 `Retry-After`;使用已有公钥恢复已注册 installation 仍受尝试速率保护,但不消耗新身份额度,也不受未绑定 installation 水位线阻断。

最小生产示例:

```bash
cat > "$HOME/.cc-switch-router/.env" <<'EOF'
CC_SWITCH_ROUTER_API_ADDR=0.0.0.0:80
CC_SWITCH_ROUTER_SSH_ADDR=0.0.0.0:2222
CC_SWITCH_ROUTER_TUNNEL_DOMAIN=example.com
CC_SWITCH_ROUTER_OWNER_EMAIL=router@example.com
CC_SWITCH_ROUTER_USE_LOCALHOST=false
CC_SWITCH_ROUTER_RESEND_API_KEY=re_xxx
CC_SWITCH_ROUTER_RESEND_FROM=noreply@example.com
EOF
```

Client 生命周期通知使用持久化 outbox、固定 Resend 幂等键和离线 episode 去重,注册与离线邮件都只发送至对应 Client 当前已验证的 Owner 邮箱。关闭总开关时,Router 会推进在线状态 baseline 并抑制待发记录;以后重新启用不会补发停用期间的历史通知。多 Client 在窗口内集中注册或离线时会按 Owner 合并为 digest。Offline lane 使用独立的单收件人/全局 `10/50` 小时额度,registration lane 使用独立的 `3/10` 小时额度,两者互不占用。未完成的 outbox 会持续保留,已发送、dead-letter、取消和抑制记录保留 30 天供审计。

Share Market 与 Client Market 对免费和付费商品统一采用供应商准入策略,默认均为白名单。供应商先按买家邮箱建立信任关系,再按产品允许访问；付费商品还必须按买家和供应商授予 USD 有限或无限信用额度。切换到黑名单模式必须显式确认风险,未知买家只能使用免费商品,或在供应商另行开启有限公共额度后租用付费商品；公共额度不能设为无限。

付费商品共用账户级后付费赊账：每项服务先享受 12 小时健康时长试用,之后只按 Router 观测到的健康服务秒数累计固定 USD 每日费用。同一买家和供应商共用一个 USD 余额；有限额度使用达到 80% 时向相关 Client 公开聊天室写入系统预警,用满后生成聚合账单并暂停相关服务。账单按固定 `1 USD = 7 CNY` 同时提供美元与人民币金额,CNY 只用于展示而不形成独立账户。无限额度不自动出账,供应商可主动要求清账；买家也可主动清账,最后一项服务结束时剩余余额会自动出账。Router 不经手资金,付款声明仍需供应商确认到账；逾期声明或争议不会自行解除市场赊账限制。

Client 公开聊天室与 `installation.id` 一一对应,只为已验证 Owner 的 Client 建立；同一 Client 下的所有 Share 共用这一房间,不存在 Share 独立聊天室。历史消息公开可读,发送真人消息必须使用 Router 登录 Session;普通用户 API Token 不能发送。匿名访客的最近聊天室和已读游标只保存在当前浏览器,登录后会一次性合并到服务端用户记录。非 Owner 真人消息在同一聊天室内从第一条消息开始使用固定 60 秒窗口聚合,窗口内每条消息都完整写入同一封 Owner 邮件;Owner 自己的消息和系统消息不会触发聊天邮件。消息与邮件事件在同一 SQLite 事务落库,后台使用固定 Resend 幂等键、claim lease、重试和 dead-letter。Client 被清理后聊天室转为公开只读归档并保留 60 天,同一 Client 在期限内恢复时沿用原房间。

Share Market、Client Market 与统一账务的关键事件通过持久化 outbox 写入对应 Client 公开聊天室。租用双方的完整邮箱、账单金额、收款方式与联系方式、付款 reference/note、凭证 URL、争议或回收原因以及安全的原始错误均公开展示；系统消息引用的同源收款图片随消息公开并在消息保留期内防止清理,未发布图片仍需 Owner 或账单买方身份。API Key、OAuth/Session token、Cookie、Authorization、密码、secret、私钥和 SSH/lease 凭据禁止进入 Market 源事件和聊天室 payload；后端在持久化前拒绝敏感字段与 query/fragment/userinfo 带凭据的 URL,并替换外部错误或备注中的凭据片段,前端渲染时再执行一次同类过滤。`PaymentMethod.token` 只允许表达 `USDT`/`USDC` 资产符号。验证码、安全通知、Client 注册/离线生命周期邮件和真人聊天提醒邮件仍保留,Market/Billing 业务事件本身不再发送交易邮件。

旧 `/v1/board/*` 数据不迁移也不删除;GET 在一个兼容版本内保持只读,POST/置顶/精选/删除均返回 HTTP `410 Gone`。旧 `CC_SWITCH_ROUTER_BOARD_*` 和 Board Telegram 开关仅作为兼容配置保留,不影响 Client 聊天室。

Setup 完成通知采用 Router-first 发布顺序:先部署支持 `POST /v1/installations/setup-completed` 的 Router,再升级 Server。新 Server 在 setup 成功后显式提交签名完成事件;尚未升级的旧 Server 首次 claim Client tunnel 时,Router 会创建临时 fallback 并等待固定 30 分钟,宽限期内若收到显式事件就由显式事件接管,否则才发送 legacy 注册通知。fallback 仅覆盖刚注册并很快 claim tunnel 的 Client,旧 installation 重连不会触发。所有受支持 Server 版本都已实现显式上报且最旧版本退出后,应删除该兼容桥。

升级兼容:`CC_SWITCH_ROUTER_CLIENT_ALERT_EMAILS` 与 `CC_SWITCH_ROUTER_CLIENT_OFFLINE_NOTIFY_OWNER` 已废弃并被忽略,即使旧 `.env` 仍保留这些键也不会生效。Owner-only 通知仅由 `CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED` 总开关控制;旧部署若保持该开关为 `true`,升级后收件人会切换为对应 Client 当前已验证的 Owner。

### 启动

```bash
cc-switch-router
```

查看帮助:

```bash
cc-switch-router help
```

调整日志级别:

```bash
RUST_LOG=debug cc-switch-router
```

### 验证部署

```bash
curl http://127.0.0.1/v1/healthz
# {"ok":true}
```

控制台:`http://127.0.0.1/`

`/` 和 `/v1/dashboard` 默认公开可读,不需要登录。

dashboard 当前行为:

- 未登录时 share 表格中的 API key 默认脱敏
- owner 或 `shared_with_emails` 中的邮箱登录后,可看到对应 share 的 API key 明文
- 页脚 `PAGE ONLINE` 右侧在 free plan 且 Resend 返回 `x-resend-daily-quota` 时,会显示 `RESEND USAGE xx%`
- Resend 用量由服务端每 10 分钟主动请求一次并缓存;若响应头不存在,则页脚只显示 `PAGE ONLINE`

邮件登录相关端点:

- `POST /v1/auth/email/request-code` 请求邮件验证码
- `POST /v1/auth/email/verify-code` 校验验证码并签发 access / refresh token
- `POST /v1/auth/session/refresh` 刷新会话
- `GET /v1/auth/session/me` 查询当前浏览器登录态

`GET /v1/public/map-points` 返回公开地图所需的点位数据,其中 `clients` 是按国家质心聚合后的地图点数组,每个点包含 `count`;`clientCount` 是符合条件的真实活跃 client 总数,两者可能不相等。

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

Router 收到 `SIGTERM` 后先停止 HTTP 接入并最多排空 30 秒,再关闭 SSH
listener。日志使用 append 模式;生产环境应由 `logrotate` 或 journald 负责轮转,
不要在重启脚本中截断日志。

## 当前限制

**协议与功能**

- 仅实现 HTTP/WebSocket tunnel,不支持任意 TCP 转发
- 邮件验证码登录是基于服务端持久化 session 的 bearer token,不是 JWT。验证码按邮箱、installation 和用途隔离；同一邮箱的多设备验证码可并存，校验必须使用发码时的 installation 身份
- Resend 用量展示依赖官方响应头 `x-resend-daily-quota`;该 header 通常只对 free plan 返回,不返回时页脚不会显示用量

**Share 数据一致性**

- 设备私钥由 `cc-switch-server` 以本地文件方式保存(`server.json` 内的 `router.identity`),未接入系统安全存储
- share 用量同步为「事件驱动最终一致」,由 `cc-switch-server` 在创建、状态变更、用量变更、删除时异步上报
- `cc-switch-server` 端 share 同步已做短延迟批量聚合,降低高频请求噪音
- share owner / `shared_with_emails` ACL 以 `cc-switch-server` 推送为准,`cc-switch-router` 负责持久化、鉴权和 dashboard 脱敏控制

**运行与清理**

- `cc-switch-router` 会定时清理超过保留期的历史 lease,以及状态为 `expired` / `deleted` 的陈旧 share 记录
- 当请求经 Cloudflare 代理进入时,free share 限流会基于可信的 `CF-Connecting-IP` 识别真实用户 IP;直连源站时会回退到 socket peer IP,防止伪造头绕过限制
- 后台任务在进程关停时被直接 abort,不做单独排空

**已知架构限制**

- 所有 SQLite 访问(含只读)串行通过单个 `Mutex<Connection>`,这是当前主要的并发瓶颈
- 数据库迁移无版本表,依赖 `CREATE TABLE IF NOT EXISTS` 与列存在性探测,每次启动全量重跑,无回滚路径
- 默认用户 API token 以明文列存储以支持 UI 重复展示;数据库泄露等同于活跃 token 泄露
- 注册限流的内存 token bucket 无持久化,进程重启后短时间内尝试速率保护失效

详见 [ARCHITECTURE.md](ARCHITECTURE.md)。
