# cc-switch-router

官网 **[tokenswitch.org](https://tokenswitch.org)** · 系统文档 **[docs.tokenswitch.org](https://docs.tokenswitch.org)** · Client **[cc-switch-server](https://github.com/Xiechengqi/cc-switch-server)**

TokenSwitch 的公共汇聚层。为 `cc-switch-server` 实例提供公网子域名与反向隧道，并在 Client + Router 边界内承载 Share Market、Client Market 与多区域 Router 联邦。旧独立 Token Market 不再是运行时依赖；未来外部容量平台只能通过中性的、签名的 Gateway 入口接入。

| Region | 24h usage |
| --- | --- |
| [japan](https://jptokenswitch.cc) | ![japan 24h](https://jptokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |
| [singapore](https://sgptokenswitch.cc) | ![singapore 24h](https://sgptokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |
| [hongkong](https://hktokenswitch.cc) | ![hongkong 24h](https://hktokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |
| [usa](https://ustokenswitch.cc) | ![usa 24h](https://ustokenswitch.cc/v1/public/embed/global.svg?period=24h&theme=light) |

同一份数据的 JSON 出口是 `GET /v1/public/usage/global?period=24h`（另有 `7d` / `30d`），给需要跨区域汇总的调用方使用 —— 例如官网把四个 Region 的 24h 用量相加并展开各模型。响应是纯聚合量：总量、输入/输出/缓存分项、逐模型明细、逐桶趋势、活跃 Share 与 Client 数，不含邮箱、Share、账号或金额。默认返回**全部**模型行；`?models=N` 才截断。跨区域汇总必须先合并再截断，否则每个 Region 的长尾模型会被静默丢掉，明细之和不再等于总量。

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
                  │  libSQL 业务库 + 本地 metrics 库    │
                  └────────────────────────────────────┘
                                  ▲
                                  │ SSH reverse tunnel
                                  │
                         cc-switch-server 实例
```

单进程同时承载三个职责:

- **HTTP 服务** — API 端点 + 基于 Host subdomain 的反向代理 + 内嵌前端,共用同一端口
- **SSH 服务** — 基于 `russh` 的 reverse forwarding,一次性密码认证
- **数据存储** — 业务库可使用本地 libSQL 或 Turso Cloud Embedded Replica；metrics 使用独立本地数据库；Server 结构化审计日志只写 Router 自有 JSONL/压缩文件，不进入业务库

核心依赖:`axum`、`russh`、`libsql`、`tokio`、`reqwest`

## 客户端

**`cc-switch-server` 是本 Router 的唯一客户端。** 它是无桌面依赖的 Rust server,自身为 Claude Code / Codex CLI / Gemini CLI 提供本地反代,并通过本 Router 获得公网可达性。

早期的 `cc-switch` Tauri 桌面版已不再作为客户端,相关兼容代码已移除,详见 [MIGRATION.md](MIGRATION.md)。

远程主机上的部署由仓库内 `install-client.sh` 负责,它会下载 `cc-switch-server` 二进制并完成初始化。Router 的 `/install-client.sh` 会在请求时写入 Settings 中当前选择的 Server Release；默认值是 `latest`，也可固定为已经通过 Server 手动发布流程生成的 7 位 Commit Release。脚本严格识别 AMD64/ARM64，同时下载同一 Release、同一下载源的 binary 与 `.sha256`，在覆盖现有文件前完成 checksum；固定 Commit 时还会检查 staged binary 的 `version --json`。任一步失败都会终止，不会回退 `latest`。此设置只影响之后下载的安装脚本，不会修改已安装 Server，Server 自升级仍保持使用 `latest`。

Client Market 的首次开通会自动调用该脚本并启动一次进程；systemd unit 使用 `Restart=no` 且不启用开机启动，OpenRC 服务不配置 respawn 且不加入默认 runlevel，没有受支持的服务管理器时仅执行一次 `nohup`。

首次开通完成后,Router **不会**因为 tunnel 离线而通过 SSH 检查、启动或重启远端 `cc-switch-server`。Router 只保留连接状态观测、心跳、离线告警和页面提示；进程生命周期由 Client owner 负责，离线时应登录 Host 手动排查并启动服务。只有用户明确发起的首次开通、升级、清理或回收操作可以执行对应远端命令。清理失败的后台重试只继续既有的清理流程，不会拉起 Client 服务。

客户端与 Router 之间的注册、lease、建链、控制平面与身份注入契约,见 [PROTOCOL.md](PROTOCOL.md)。

Clients 页的每个 client 条目提供「控制台」和「终端」两个入口:两者都以 iframe 弹窗打开该 client 自己的 Web 界面,经由 client tunnel 转发,登录态由 client 自身管理。终端入口在 client web URL 上追加 `?view=terminal&embed=1`,client 登录后直接落到终端视图,并以无边框形态只显示终端本身(页头、连接状态和「结束会话」按钮都由弹窗自己的标题栏取代；结束会话直接在终端里 `exit` 即可)；若该 client 关闭了 `enableWebTerminal`,它会自行回落到默认视图。控制台与终端各自独立成窗,可同时打开。

## 边界策略

Client Web tunnel:静态资源和明确列出的登录/OAuth 回调公开;其余 `/web-api/*` 默认要求 owner/admin 身份,Router 鉴权后向 client 注入可信身份头。`/api/*`、`/v1/*`、`/_ctl/*` 和 `/_share-router/*` 不通过 client web tunnel 暴露。流式管理接口必须使用 `Authorization` header,不接受 query-string token。

## 用户统一模型入口

每个区域额外提供一个可选的共享入口 `https://api.<TUNNEL_DOMAIN>`。用户在 Clients 页「我的」分页的「模型中枢」中维护精确的 `(App, 请求模型) → Share` 映射，同一把 Router API Key 可据此调用多个当前有权限的 Share。App 固定为 Claude、Codex、Gemini；模型名区分大小写且不会被 Router 改写。

该入口不会替代 Share 子域名。`https://<share-subdomain>.<TUNNEL_DOMAIN>` 的直连调用继续生效，用户不配置任何模型路由时也不受影响。统一入口不存在默认 Share、隐式选择、fallback 或跨 Share 重试；映射只负责选择目标，不能授予权限。每次调用仍重新检查目标 Share 的 Owner / ShareTo / Free 权限、App 开启状态与在线路由，并复用原有 Share 代理链，所以地图、Share 侧边栏、请求日志、用量、限额和账务仍全部归属目标 Share。

配置控制面为 Session 鉴权的 `GET/PUT /v1/me/model-routing`，推理面接受同一用户 API Key 的 `Authorization: Bearer`、`x-api-key` 或 `x-goog-api-key`。完整路径与错误契约见 [PROTOCOL.md](PROTOCOL.md)。

## API 端点

API 路由按域分组概览如下,协议细节见 [PROTOCOL.md](PROTOCOL.md)。

| 域 | 路径数 | 认证方式 | 代表端点 |
|---|---:|---|---|
| `/v1/client-market/*` | 30 | 用户 Session | `hosts`、`quotes`、`quotes/:id/commit`、`providers`、`my-rentals`、`terminal/ws` |
| `/v1/admin/*` | 约 46 | Session + admin 判定 | `settings`、`settings/validate`、`version`、`upgrade`、`metrics/*`、`alerting/*`、`audit`、`proxy/share-requests/force-release`、`logs/router/tail`、`market-billing/disputes`、`market-billing/binance-reconciliation` |
| `/v1/shares/*` | 17 | installation bearer / Ed25519 签名 | `claim-subdomain`、`sync`、`batch-sync`、`descriptor-batch-sync`、`pending-edits`、`edit-ack`、`edit-events`、`runtime-refresh`、`heartbeat`、`prune` |
| `/v1/installations/*` | 12 | Ed25519 签名 / bearer | `register`、`heartbeat`、`audit-events/batch`、`setup-completed`、`report-status`、`client-tunnel`、`client-tunnel/claim`、`bind-owner-email` |
| `/v1/server-logs/*` | 4 | 公开读 / 用户 Session / Router owner | `meta`、`events`、`export`、`clients/:installation_id/live-tail`；匿名只读最近 5 分钟公开投影，用户读取自有 Client，Router owner 读取全部 Client 并可按需拉取在线诊断 |
| `/v1/clients/:installation_id/logs` | 1 | 公开读 / 已验证 Client owner | 从在线 Client 拉取脱敏进程日志；Client owner 最多 100 行，匿名、非 owner 用户和非 owner 管理员最多 10 行 |
| `/v1/chat/*` | 9 | 公开读 / Session 写 | `clients/:installation_id/room`、`rooms/:room_id/messages`、`rooms/:room_id/stream`；不存在 Share 独立房间 |
| `/v1/markets*`、`/v1/market/*`、`/_market/proxy/*` | 退役 | 无 | 统一返回 `410 Gone`；不会创建旧 Market session、host 或 proxy |
| `/v1/share-market/*` | 9 | 公开 catalog / 用户 Session | `listings`、`owned-shares`、`seats/:id/rent`、`subscriptions/:id/release`、`force-revoke`；停止挂售后无活跃租约可再次 `POST listings` |
| `/v1/market-access/*` | 12 | 用户 Session / scoped API Token | `dashboard`、`inbox-summary`、`policies`、`counterparties/batch`、准入申请批准/拒绝/取消、买家授信与公共额度 |
| `/v1/market-billing/*` | 15 | 公开 `config` / 用户 Session | `config`、`dashboard`、`supplier-profiles`、`accounts/:id/settle`、`request-settlement`、`accounts/:id/invoices`、付款声明/确认/拒绝与争议、`invoices/:id/binance-intent` |
| `/v1/gateway/*`、`/v1/gateways/*` | 5 | Ed25519 签名(`x-cc-gateway-*`) | `register`、`shares`、`shares/headroom`、`shares/feedback`、`request-logs/batch`；签名绑定原始 body，owner email、Free 与 ShareTo 均不授权，新 grant contract 前普通 Share 整体 fail-closed |
| `/v1/auth/*` | 5 | 公开 / Session | `email/request-code`、`email/verify-code`、`session/refresh`、`session/me`、`session/logout` |
| `/v1/tunnels/*` | 4 | Ed25519 签名 | `lease`、`lease/renew`、`activate`、`state` |
| `/v1/account/*` | 5 | Session | `payment-profile`、`payment-assets/:id`、`binance-auto-settlement` 绑定/复验/停用 |
| `/v1/public/*` | 4 | 公开 | `map-points`、`network-stats`、`embed/global.svg`、`embed/usage/:user_id` |
| `/share-api/*` | 4 | 子域名上下文,Session 可选 | `context`、`share`、`auth/me`、`share/settings` |
| `/v1/dashboard/*`、`/v1/me/*` | 14 | Session | `dashboard`、`presence`、`ux-events`、`me/api-token`、`me/model-routing`、`me/shares`、`me/usage-card`、`me/usage/consumer`、`me/usage/provider`、`me/notifications`、`me/notifications/telegram/bind-link` |
| 其余单例 | 约 16 | 混合 | `healthz`、`regions`、`announcement`、`map-display`、`client-tunnel/subdomain-availability`、`integrations/telegram/webhook`、`_gateway/proxy/*`、`*path`(前端与反代 catch-all) |

## 管理设置与运维

管理员页面按职责拆分为两个入口：`/settings/` 只管理配置，`/operations/` 承载版本与服务控制、Router 日志、通知投递历史和管理员审计。Settings 的受管环境变量分为 General & Display、Connectivity、Data & Lifecycle、Identity & Security、Notifications、Observability、Marketplace 七个配置域；Binance settlement 位于 Marketplace，地图和公告留在 General & Display，通知渠道健康检查留在 Notifications。

Settings 使用 revision 化的三段式 API：`GET /v1/admin/settings` 返回 schema、持久化值、运行时有效值和 revision；`POST /v1/admin/settings/validate` 在不写盘的情况下校验整组修改；`PATCH /v1/admin/settings` 要求相同的 `expectedRevision` 后才原子替换 `.env`。并发修改返回 `SETTINGS_REVISION_CONFLICT`，前端会加载最新版本并保留待复核草稿。地图与公告也有各自的 revision，避免多个管理员互相覆盖。

Settings 管理的 `.env` 是 Router 配置的唯一权威来源，启动时会覆盖进程中预先存在的同名变量，全部字段均可通过 Web 修改。热更新字段的运行值直接读取当前 `DynamicSettings`；绕过 API 手工修改 `.env` 时，页面会保留实际内存值并标记等待重启，不会把文件值误报为已生效。Secret 只返回是否已配置，API 从不返回明文；清除 Secret 必须使用显式操作。`.env` 与备份写入采用 `0600` 权限、临时文件 fsync 和同目录原子 rename，需要重启的字段保存后会持续显示 pending restart，直到新进程读取该值。

配置契约由 Rust 单元测试和前端审计共同保护。修改 Settings 字段时需运行：

```bash
cargo test admin::settings
cd frontend
npm run audit:settings-i18n
npm run audit:settings-contract
```

Share 请求在请求体、响应头、首业务事件、业务空闲、下游背压和绝对生存时长六个阶段均有独立边界；响应由后台泵读取，因此浏览器或 API 调用方停止消费 Body 时仍会释放并发。10 秒周期的看门狗会按唯一 lease 幂等回收异常残留并触发通用告警，绝不重启 Router。管理员兜底接口 `POST /v1/admin/proxy/share-requests/force-release` 只接受 `requestId` 或 `shareId` 其中一个，可附带 `reason`，每次实际释放都会取消上游任务、增加 metrics 计数并写入 admin audit。

## 二进制部署

### 准备发布包

GitHub Actions 会在 `master` 分支自动构建 Ubuntu AMD64 二进制,并更新 `latest` Release。部署时直接下载 release binary:

```bash
wget https://github.com/xiechengqi/cc-switch-router/releases/download/latest/cc-switch-router-linux-amd64 -O /usr/local/bin/cc-switch-router && chmod +x /usr/local/bin/cc-switch-router
```

前端资源在编译期由 `build.rs` 内嵌进二进制,`cargo build --release` 前必须先执行 `(cd frontend && npm ci && npm run build)`。

### 环境变量

默认配置文件路径:`$HOME/.cc-switch-router/.env`

启动时如果这个文件不存在,`cc-switch-router` 会自动生成默认 `.env`,然后按该文件加载配置。`.env` 中的受管配置优先于进程中预先存在的同名变量，后续通过 Web Settings 统一修改。

可用环境变量:

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CC_SWITCH_ROUTER_API_ADDR` | `0.0.0.0:80` | HTTP 监听地址 |
| `CC_SWITCH_ROUTER_SSH_ADDR` | `0.0.0.0:2222` | SSH 监听地址 |
| `CC_SWITCH_ROUTER_TUNNEL_DOMAIN` | `0.0.0.0:8787` | 公共 tunnel 域名；统一模型入口固定使用其中的保留标签 `api` |
| `CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR` | `{TUNNEL_DOMAIN}:{SSH_PORT}` | 下发给客户端的 SSH 地址(Cloudflare 代理时填源站 IP:端口) |
| `CC_SWITCH_ROUTER_SSH_INACTIVITY_TIMEOUT_SECS` | `300` | 入站 SSH 无流量超时，范围 30-3600 秒；必须覆盖完整 keepalive 失败窗口 |
| `CC_SWITCH_ROUTER_SSH_KEEPALIVE_INTERVAL_SECS` | `30` | 入站 SSH 无流量时的 keepalive 周期，范围 5-300 秒 |
| `CC_SWITCH_ROUTER_SSH_KEEPALIVE_MAX` | `3` | 未响应 keepalive 上限，范围 1-10 |
| `CC_SWITCH_ROUTER_SSH_CHANNEL_OPEN_TIMEOUT_SECS` | `15` | 等待 client 确认 forwarded TCP 通道的超时，范围 1-120 秒 |
| `CC_SWITCH_ROUTER_SSH_BRIDGE_WRITE_STALL_TIMEOUT_SECS` | `300` | 有待写数据但无写入进展时关闭 bridge，范围 30-3600 秒；双向纯空闲不触发 |
| `CC_SWITCH_ROUTER_SSH_BRIDGE_HALF_CLOSE_IDLE_TIMEOUT_SECS` | `300` | 单向 EOF 后剩余方向无进展的超时，范围 30-3600 秒 |
| `CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS` | `2048` | 全局等待通道建立与活动 bridge 的 forwarded TCP 连接总上限，范围 1-65536 |
| `CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL` | `256` | 单 SSH 隧道等待通道建立与活动 bridge 的连接总上限，范围 1-4096 且不得超过全局上限 |
| `CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_TIMEOUT_SECS` | `30` | 下游请求体读取超时，范围 5-300 秒；请求体完成后才占用 Share 并发 |
| `CC_SWITCH_ROUTER_PROXY_RESPONSE_HEADER_TIMEOUT_SECS` | `120` | 上游响应头等待超时，范围 5-600 秒 |
| `CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS` | `120` | Share 流首个协议业务事件超时，范围 5-600 秒；SSE 注释与 keepalive 不会续期 |
| `CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS` | `900` | Share 流后续协议业务事件空闲超时，范围 30-3600 秒；SSE 注释与 keepalive 不会续期 |
| `CC_SWITCH_ROUTER_PROXY_DOWNSTREAM_STALL_TIMEOUT_SECS` | `120` | 下游停止消费有界响应缓冲区时的终止超时，范围 5-600 秒 |
| `CC_SWITCH_ROUTER_PROXY_MAX_REQUEST_LIFETIME_SECS` | `7200` | Share 请求绝对生存时长，范围 60-86400 秒，必须大于所有阶段超时 |
| `CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_LIMIT_MB` | `10` | 普通 API（`/v1/responses`、`/v1/messages` 等）请求体上限，范围 1-64 MB；超限在占用 Share 并发前返回 413。请求体整体驻留内存，峰值内存 ≈ 该值 × 并发请求数。该值会随每个转发请求声明给 Client（`x-cc-switch-ingress-body-limit`），Client 取 `min(本地上限, 声明值)`；新版 Client 本地默认已是该范围上限，因此通常只调这里即可 |
| `CC_SWITCH_ROUTER_PROXY_MEDIA_REQUEST_BODY_LIMIT_MB` | `32` | `/v1/videos/generations` 请求体上限，范围 1-256 MB，且不得小于普通 API 上限。同样随请求声明给 Client，取 `min(本地上限, 声明值)`。超过约 100 MB 时通常先被边缘代理（如 Cloudflare）拦下；大体积上传也可能先撞 `CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_TIMEOUT_SECS` |
| `CC_SWITCH_ROUTER_PROXY_IMAGE_REQUEST_BODY_LIMIT_MB` | `48` | `/v1/images/generations` 与 `/v1/images/edits` 请求体上限（含内联 base64 附件），范围 1-256 MB，且不得小于普通 API 上限。同样随请求声明给 Client，取 `min(本地上限, 声明值)`。注意 multipart 形式的 `/v1/images/edits` 另受 Client 内容层限制（单张 20 MiB、合计 32 MiB、最多 16 张，超限返回 400），该值调到 32 MB 以上不会放宽这条路径 |
| `CC_SWITCH_ROUTER_OWNER_EMAIL` | `router@{TUNNEL_DOMAIN}` | Client Market 默认选中的官方 Host Provider 邮箱 |
| `CC_SWITCH_ROUTER_USE_LOCALHOST` | `false` | 为 `false` 时 tunnel URL 使用 `https://` |
| `CC_SWITCH_ROUTER_LEASE_TTL_SECS` | `60` | Tunnel lease 有效期(秒);已连接 client 使用签名续期 API 原连接续期,不按该周期重建 SSH |
| `CC_SWITCH_ROUTER_DATA_DIR` | `$HOME/.cc-switch-router` | Router 自有本地文件目录,包含图片结果、SSH known_hosts 等 |
| `CC_SWITCH_ROUTER_DB_MODE` | `local` | 业务数据库模式:`local` 或 `turso` |
| `CC_SWITCH_ROUTER_DB_PATH` | `$HOME/.cc-switch-router/cc-switch-router.db` | local 模式的 libSQL 文件,或 turso 模式的 Embedded Replica 文件 |
| `CC_SWITCH_ROUTER_TURSO_URL` | 空 | turso 模式必填的 `libsql://` 或 `https://` 数据库 URL;不得携带凭据、query 或 fragment |
| `CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN` | 空 | turso 模式必填的数据库 Token;Settings API 只返回是否已配置,不返回明文 |
| `CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS` | `60` | Embedded Replica 拉取 Turso 已提交 frame 的周期,范围 1-3600 秒 |
| `CC_SWITCH_ROUTER_METRICS_DB_PATH` | `$HOME/.cc-switch-router/cc-switch-router-metrics.db` | 独立本地 metrics 数据库;不会同步到 Turso |
| `CC_SWITCH_ROUTER_METRICS_ENABLED` | `true` | 是否采集 Host、Router、Client 和 LLM metrics；修改后需重启 |
| `CC_SWITCH_ROUTER_METRICS_RETENTION_DAYS` | `7` | metrics 采样历史保留天数；事故历史使用独立配置 |
| `CC_SWITCH_ROUTER_METRICS_SAMPLE_INTERVAL_SECS` | `5` | metrics 采样与条件判断间隔；修改后需重启 |
| `CC_SWITCH_ROUTER_CLOCK_MONITOR_ENABLED` | `true` | 是否用 HTTPS Date 仲裁持续观测 Router 主机时钟；只观测和告警,不会修改系统时间 |
| `CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS` | `60` | 时钟健康探测间隔,范围 15-3600 秒 |
| `CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS` | `4` | 每个 HTTPS 时间源的完整请求超时,范围 1-15 秒 |
| `CC_SWITCH_ROUTER_CLOCK_SOURCES` | Cloudflare、Apple、AWS 三路 HTTPS URL | 逗号分隔的 3-5 个不同 HTTPS host；至少两路相符才形成可信偏差样本 |
| `CC_SWITCH_ROUTER_ALERTING_ENABLED` | `true` | 是否为新事故流转创建 IM 投递；事故本身始终持久化，可在 Settings 热更新 |
| `CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS` | `1800` | 未确认活跃事故的提醒间隔，范围 60 秒至 7 天 |
| `CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS` | `90` | 已恢复事故、流转、投递尝试、渠道测试和已完成 Client 信号的保留天数 |
| `CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED` | `false` | 启用 Telegram Bot 告警；同时要求 Bot Token 和 Chat ID |
| `CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN` | 空 | `@BotFather` 签发的 Token；Settings API 不回传明文 |
| `CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID` | 空 | Telegram 私聊、群组、超级群组或频道 ID |
| `CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID` | 空 | 论坛模式超级群组的可选 `message_thread_id` |
| `CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY` | `warning` | Telegram 最低投递级别：`info`、`warning` 或 `critical` |
| `CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS` | `300` | 清理任务执行间隔(秒) |
| `CC_SWITCH_ROUTER_LEASE_RETENTION_SECS` | `86400` | 过期 lease 保留时长(秒) |
| `CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS` | `30` | Share 请求记录和图片请求历史保留天数,范围 1-365;不影响累计 Token 用量 |
| `CC_SWITCH_ROUTER_SERVER_LOG_INGEST_ENABLED` | `true` | 是否接收 Server 结构化审计日志；修改后需重启 |
| `CC_SWITCH_ROUTER_SERVER_LOG_DATA_DIR` | `$CC_SWITCH_ROUTER_DATA_DIR/server-logs` | Server 日志 JSONL、gzip 段、cursor 与可重建 manifest 目录；修改后需重启 |
| `CC_SWITCH_ROUTER_SERVER_LOG_RETENTION_DAYS` | `7` | Server 日志文件保留天数，范围 1-90；修改后需重启 |
| `CC_SWITCH_ROUTER_SERVER_LOG_MAX_TOTAL_MIB` | `1024` | Server 日志文件总容量上限 MiB，至少保留逻辑上最新的已接收事件文件；修改后需重启 |
| `CC_SWITCH_ROUTER_SERVER_LOG_PUBLIC_ENABLED` | `true` | 是否允许匿名查看最近 5 分钟的脱敏公开投影；可在 Settings 热更新 |
| `CC_SWITCH_ROUTER_CLIENT_STALE_SECS` | `3600` | client 超过该时间未心跳时标记离线,并清理其 share、lease 与内存路由 |
| `CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS` | `21600` | 离线 client 的 installation 记录保留时长,超时后删除;必须 >= `CLIENT_STALE_SECS` |
| `CC_SWITCH_ROUTER_CLIENT_SERVER_RELEASE` | `latest` | 新下载的 `install-client.sh` 使用的 GitHub Release；可热更新为 `latest` 或已存在且双架构 binary/checksum 完整的 7 位 Commit Release |
| `CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE` | `60` | 单可信来源每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST` | `20` | 单可信来源允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE` | `600` | Router 全局每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST` | `200` | Router 全局允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE` | `10` | 单公钥每分钟持续注册尝试速率 |
| `CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST` | `3` | 单公钥允许的短时注册尝试突发量 |
| `CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS` | `600` | 来源/公钥尝试计数器的空闲释放时间(秒) |
| `CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS` | `8192` | 内存中同时保留的来源尝试计数器上限 |
| `CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS` | `16384` | 内存中同时保留的公钥尝试计数器上限 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT` | `30` | 单来源 10 分钟内持久化每类新身份的额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT` | `100` | 单来源每小时持久化每类新身份的额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT` | `300` | 单来源每日持久化每类新身份的额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT` | `300` | Router 全局 10 分钟内持久化每类新身份的额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT` | `1000` | Router 全局每小时持久化每类新身份的额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT` | `5000` | Router 全局每日持久化每类新身份的额度 |
| `CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK` | `50000` | 未绑定 Owner 的 installation 记录达到该水位后暂停新身份准入 |
| `CC_SWITCH_ROUTER_RESEND_API_KEY` | 空 | Resend API Key,用于验证码、Client 生命周期/聊天室邮件和 dashboard 用量读取;未配置时禁止发送聊天消息 |
| `CC_SWITCH_ROUTER_RESEND_FROM` | 空 | 邮件发件人,可填裸邮箱或 `TokenSwitch <noreply@example.com>`;裸邮箱会自动显示为 `TokenSwitch <邮箱>` |
| `CC_SWITCH_ROUTER_RESEND_FROM_NAME` | `TokenSwitch` | `CC_SWITCH_ROUTER_RESEND_FROM` 为裸邮箱时使用的发件人显示名 |
| `CC_SWITCH_ROUTER_RESEND_REPLY_TO` | 空 | 验证码、Client 生命周期与聊天室邮件的 Reply-To |
| `CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED` | `true` | Client 注册/离线邮件总开关;通知仅发送至对应 Client 当前已验证的 Owner 邮箱 |
| `CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS` | `180` | 连续缺少可信签名心跳多久后确认离线;安全下限为 180 秒 |
| `CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS` | `120` | 离线 Client 心跳持续稳定多久后才结束原离线 episode；不会启动或重启进程 |
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
| `CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED` | `false` | 用户通知 Telegram Bot 总开关;关闭时账户页只能选择邮件 |
| `CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN` | 空 | 用户通知 Bot 的 `botfather` token;与运维告警渠道的 Telegram 配置相互独立 |
| `CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE` | `polling` | 接收 `/start` 的方式:`polling` 或 `webhook` |
| `CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET` | 空 | webhook 模式下校验 `x-telegram-bot-api-secret-token` 的共享密钥;未设置时 webhook 不启用 |
| `CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS` | `900` | 绑定深链 token 有效期(秒) |
| `CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT` | `10` | 单用户每小时 Telegram 通知硬上限;与邮件额度互不占用 |
| `CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT` | `50` | Router 全局每小时 Telegram 通知硬上限 |
| `CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS` | `300` | 邮件验证码有效期(秒) |
| `CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS` | `60` | 同邮箱 / 设备发验证码冷却(秒) |
| `CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS` | `1800` | Access token 有效期(秒) |
| `CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS` | `2592000` | Refresh token 有效期(秒) |
| `CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS` | `5` | 单挑战最大输错次数 |
| `CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT` | `30` | 单邮箱每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT` | `20` | 单 IP 每小时最大发送次数 |
| `CC_SWITCH_ROUTER_AUTH_SOURCE_HOURLY_LIMIT` | `10` | 单认证来源每小时最大发送次数 |
| `CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT` | `1` | 所有 `free_access = 1` 的公开免费 Share 共用的单真实用户 IP 并发上限；v1 `forSale=Free` 只在 migration 20 的持久化迁移边界识别，不属于 active contract；设为 `0` 可关闭 |
| `CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE` | `7` | 市场账务美元兑人民币汇率（1 USD 对应的 CNY，范围 0.01-100，最多 6 位小数）；可在 Settings 热更新 |
| `CC_SWITCH_ROUTER_BINANCE_AUTO_SETTLEMENT_MODE` | `disabled` | 币安自动到账总开关：`disabled`、`shadow` 或 `enabled`；修改后需重启。`disabled` 还会持久化把全部绑定降为账户级 shadow，`shadow` 读取和匹配但不改账，并强制该阶段的新绑定保持账户级 shadow |
| `CC_SWITCH_ROUTER_BINANCE_MASTER_KEY` | 空 | 32 字节凭据加密主密钥，使用 64 位 hex 或 base64；`shadow`/`enabled` 必填，禁止与数据库一起存放；密钥本身变化后所有商家凭据也必须重新绑定 |
| `CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION` | `1` | 当前主密钥版本（1-1000000）；版本变化后，使用旧版本加密的商家凭据必须重新绑定 |
| `CC_SWITCH_ROUTER_BINANCE_API_BASE` | `https://api.binance.com` | Binance API base；生产仅接受 Binance 官方 `api`/`api-gcp`/`api1`–`api4.binance.com` 的标准 HTTPS 端口，loopback 地址仅用于测试 |
| `CC_SWITCH_ROUTER_BINANCE_PAYMENT_HOME_REGION` | Router tunnel domain（为空时 `local`） | 唯一负责轮询和结算币安付款的 Region；首期必须保证同一付款账户只有一个 home Region |
| `CC_SWITCH_ROUTER_BINANCE_POLL_INTERVAL_SECS` | `4` | 存在待付款或迟到保护账单时的轮询间隔，范围 2-60 秒 |
| `CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS` | 内置三个 `http://` 源站 | Client Market 主机 IP 情报服务,逗号分隔的 base URL,按顺序尝试。**每台登记主机的 IP 都会发送到这些端点**,应由 Router 运维方自建或交给可信任全量主机清单的一方。缺少 scheme 时按 `https://` 处理;仍使用 `http://` 时启动会打印告警。结果缓存 6 小时 |

### 统一入口 DNS 与 TLS

生产环境必须让 `api.<CC_SWITCH_ROUTER_TUNNEL_DOMAIN>` 与 Share 子域名一样解析到当前 Router HTTP 入口，并由边缘代理或源站证书覆盖该主机名。已有 `*.<TUNNEL_DOMAIN>` DNS 和通配符证书的部署通常无需新增规则；只逐条登记 Share 域名的部署必须额外添加 `api` 记录和证书 SAN。`api` 是 Router 保留标签，不能分配给 Client 或 Share。

部署后至少验证：

```bash
curl https://api.example.com/v1/healthz
curl -i -X OPTIONS https://api.example.com/v1/responses \
  -H 'Origin: https://console.example.com' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: authorization,content-type'
```

第一条应返回 Router 健康状态；第二条应返回 `204` 及匹配的 CORS 许可头。不要把 `api.<TUNNEL_DOMAIN>` 指向某个具体 Client 或 Share。

### Server 日志

Server 本地日志开启、级别为 `info` 且“日志采集”开关开启时，会记录请求 accepted/terminal、Provider 选择、重试/切换和 OAuth 刷新等脱敏结构化 INFO 审计事件，并使用 installation Ed25519 身份持续向 Router 批量上传。上传协议不包含请求/响应正文、凭据、邮箱或任意 tracing 文本；重复批次按 boot id + sequence 幂等处理，断线期间由 Server 本地 spool 有界保留。

Router 将事件按 Client 和 boot stream 写入自有 JSONL 文件，轮转段使用 gzip；不同 stream 使用独立串行槽并可并发写入。正常 ACK 顺序为事件 append/fsync、持久化 stream cursor、更新内存索引后返回；轮转、压缩、retention 和容量清理由后台 maintenance 完成。`segments.json` 保存可重建的段 manifest 与全局 `ingestOrder` 范围，重启时会与实际文件核对并重建稳定分页顺序。日志正文、事件、cursor 和 manifest 均不进入业务数据库。默认保留 7 天且总容量上限 1024 MiB；容量超限时按文件的最大 `ingestOrder` 删除逻辑上最旧的段，mtime 仅用于顺序相同时的判定，并至少保留逻辑上最新的已接收事件文件。可在 Settings 的 Server logs 分组修改采集开关、目录、保留期和容量，其中目录、保留期、容量及采集总开关需要重启；公开可见性可热更新。

全局“日志 / Log”页提供三层范围：匿名用户仅能查看事件发生时间位于最近 5 分钟内的公开投影；登录用户可查看其已验证自有 Client 的全部留存事件；Router owner 可查看全部 Client。近期公开 Client 的文件扫描使用 2 秒进程内缓存合并高频请求，事件查询仍逐次执行精确的 5 分钟窗口校验。分页 cursor 使用 HMAC 签名并绑定访问范围、可见 installation 集合和过滤条件。JSONL 导出对每个候选 segment 只扫描一次，并通过有界 channel 流式返回，不在内存累计完整导出；导出期间仅租约保护尚未读取的候选文件，不长期占用轮转/清理使用的全局文件锁，每个 segment 读入后立即释放对应租约并请求 maintenance。Client 列统一显示 Subdomain 并可打开详情侧栏；只有已登录的自有 Client 用户或 Router owner 可以导出 JSONL。兼容端点 `/v1/clients/:installation_id/logs` 通过签名控制 RPC 从在线 Client 拉取脱敏进程日志，不写入 Router 文件或数据库；已验证 Client owner 最多读取 100 行，其他访问者最多读取最近 10 行。

注册准入先使用内存中的来源、全局和公钥尝试计数器削平瞬时流量,再对真正创建的新 Client installation 与新 auth device 分别执行业务库持久化的来源/全局 10 分钟、小时和每日额度。进程重启会重置内存尝试计数器,但不会重置持久化的新身份额度。达到任一限制时接口返回 HTTP `429` 并携带 `Retry-After`;使用已有公钥恢复同类身份仍受尝试速率保护,但不消耗新身份额度。只有新 Client installation 还受未绑定 installation 水位线约束。

### Turso Cloud 模式

Turso 模式使用 Embedded Replica:读请求访问本地副本,所有写事务委派给 Turso primary,后台按配置周期拉取远端 frame。Router 不启用 offline writes,因此 Turso 不可达时写操作失败并返回 HTTP `503`/`DATABASE_UNAVAILABLE`,`/v1/healthz` 同时返回 `503`;远端恢复且下一次同步或写操作成功后健康状态恢复。

先创建一个全新的空 Turso 数据库及可写 Token:

```bash
turso db create cc-switch-router
turso db show cc-switch-router --url
turso db tokens create cc-switch-router
```

将 URL 和 Token 写入 Router 配置:

```dotenv
CC_SWITCH_ROUTER_DB_MODE=turso
CC_SWITCH_ROUTER_DB_PATH=/var/lib/cc-switch-router/router-replica.db
CC_SWITCH_ROUTER_TURSO_URL=libsql://cc-switch-router-organization.turso.io
CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN=replace-with-database-token
CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS=60
CC_SWITCH_ROUTER_METRICS_DB_PATH=/var/lib/cc-switch-router/router-metrics.db
```

部署约束:

- 只运行一个可写 Router 实例。当前没有多 Router leader election,同时写同一 Turso 数据库不在支持范围内。
- 只支持全新空业务库。首次启动原子安装 `schema/0001_baseline.sql` 并登记 checksum;非空且没有 `schema_migrations` 的数据库、未知版本或 checksum 不匹配都会拒绝启动。
- 不迁移本地旧库,也不提供 SQLite/Turso 双写。切换模式等价于为新环境选择另一套业务库。
- `CC_SWITCH_ROUTER_DB_PATH` 是当前实例独占的副本文件,不能由多个进程或节点共享。Turso 是业务数据源,metrics 数据库与 Router 本地文件仍需按普通本地状态管理。
- Token 只放在 `CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN`;不要放进 URL、命令行参数或日志。Settings 中修改数据库配置后必须重启。

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

Client 生命周期通知使用持久化 outbox、稳定幂等键和离线 episode 去重,注册与离线通知都只面向对应 Client 当前已验证的 Owner 账号。关闭总开关时,Router 会推进在线状态 baseline 并抑制待发记录;以后重新启用不会补发停用期间的历史通知。多 Client 在窗口内集中注册或离线时会按 Owner 合并为 digest。Offline lane 使用独立的单收件人/全局 `10/50` 小时额度,registration lane 使用独立的 `3/10` 小时额度,两者互不占用。未完成的 outbox 会持续保留,已发送、dead-letter、取消和抑制记录保留 30 天供审计。

### 用户通知渠道（邮件 / Telegram）

用户通知在「账户 → 通知设置」页从邮件和 Telegram 中二选一：一次只有一个投递渠道，不会同时发两份。偏好按渠道逐行保存在 `user_notification_channels`，每行带目标、Bot 身份和单调递增的 revision，并由 `user_id` 上的部分唯一索引保证同一账号最多只有一行处于选中状态；投递前会再次校验该 revision，因此解绑、换绑或切换渠道会使尚未发送的旧目标失效。Telegram 只有在 Bot 就绪且当前账号已完成绑定时才可选中，邮件始终可选。渠道偏好是账号级全局设置，不按 Client 或事件类型分别配置。

绑定流程：点击「绑定 Telegram」时 Router 生成 128 位一次性 token（只存 SHA-256），并使用已经由 Telegram `getMe` 验证的 Bot username 在新标签页打开 `https://t.me/<username>?start=<token>`。用户点 Start 后，polling 或 webhook 会先把 update 持久化到 `telegram_inbound_updates`，后台处理器再消费 `/start <token>` 并写入 Telegram 渠道行。前端轮询 `GET /v1/me/notifications`，直到 `verifiedAt` 变化；弹窗被拦截时会展示可复制的深链与 `/start <token>` 命令。token 默认 900 秒过期且一次性消费，已撤销记录仍保留到清理期以防反复换链接绕过签发限额；同一个 Bot 下一个 chat 最多绑定一个账号。绑定成功会把投递渠道自动切到 Telegram，用户随后可随时切回邮件。解绑会切回邮件，取消尚未开始的 Telegram 投递；已经进入外部 provider 调用的发送可能完成。切换渠道不会丢消息：旧渠道尚未发出的投递被取消后，其事件会重新排队到新选中的渠道。

Router owner 的配置只有三步：向 `@BotFather` 申请一个 Bot、把 token 填入 `CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN`、打开 `CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED`。Settings 只校验 BotFather token 的本地结构并立即持久化，不依赖 Telegram 当时是否可达；后台服务随后调用 `getMe` 验证身份、写入 runtime 状态并在网络故障时自动重试。Bot ID 与 username 不接受人工配置。相同 Bot ID 的 token 轮换保留现有绑定，不同 Bot ID 会使旧绑定失效并启用邮件回落。以上键可在 Settings 热更新。默认 polling 不要求入站可达性；webhook 需要公网域名和 `CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET`，回调路径 `POST /v1/integrations/telegram/webhook` 已从 IP 黑名单中豁免。两种模式使用相同的持久化 inbox 和幂等 update key，poll cursor 与一批 updates 在同一事务提交，处理失败不会跳过 update。该 Bot 与运维告警 Telegram 渠道相互独立：前者面向终端用户并按账号扇出，后者是面向单一运维会话的告警 adapter；两者可以复用 token，但配置项和状态不共享。

投递语义：同一事件只在当前选中的渠道上生成一条 `notification_deliveries`，冻结载荷、重试和结束状态。小时额度统计真实 started attempt 与仍有效的 reservation，不在 outbox 生成时预占；因此 Telegram 不会消耗或阻塞 Resend 额度。Telegram 暂时不可用时自动回落邮件；注册通知固定只走邮件，因为其正文包含首次登录口令提示。Bot 明确报告 chat 不可达时，当前 attempt 失败、仅匹配当前 Bot/目标的绑定被置为 invalid，并且原事件只重新排队一次邮件回落。Operations 中的投递历史只展示渠道、脱敏目标、结构化失败类型和阻断原因，不返回 Bot token 或原始 chat id。

Telegram 消息使用独立于邮件的排版：`parse_mode=HTML`，按严重级别加彩色徽标（离线 🔴、事故 🚨、提醒 🟠、成功 🟢、信息 🔵），字段以 `<b>` 标签加 `<code>` 值呈现，URL 保留裸链以便 Telegram 自动识别，末尾附操作链接与通知设置入口。冻结的 Telegram 载荷带 `payload_version`（2 = HTML），升级前入队的纯文本消息仍按纯文本发送；截断按标签边界进行，投递前还会校验标签闭合，若 Telegram 仍判定解析失败则自动降级为纯文本重发一次，保证告警不会因排版而丢失。

Telegram 连接失败会被分类为 DNS 解析失败、地址不可达、超时、TLS、Token 无效或 polling 冲突等稳定错误码。Router 会短暂缓存同一主机的 DNS/IPv4 TCP 探测结果，并在 Settings、账户通知、Metrics 和测试 API 展示可操作提示及脱敏技术详情；当 DNS 返回不可达地址时会明确建议检查主机 DNS（可尝试 `8.8.8.8` 或 `1.1.1.1`），而不是只显示底层 HTTP 传输异常。Polling 会用 `getUpdates`，Webhook 会每分钟用 `getMe` 做低频健康探测；成功请求会清除诊断缓存并重新入队临时阻断的 Telegram 通知，避免修复网络后继续展示旧结果或丢失待发送消息。

### LLM 性能指标

Share 卡片中的 `TTFT/TPS` 取该卡片最近 10 条请求记录，并分别计算有效样本的算术平均值。TTFT 只接受成功且完整结束、具有合法首 Token 时间的流式请求；TPS 还要求 Server 已观测到最终 usage，并按单次请求的 `output tokens / ((总延迟 - TTFT) / 1000)` 计算，不包含 input、cache 或 reasoning token。健康检查、中断请求、未结束流和无效时间数据不参与计算；TTFT 与 TPS 的有效样本彼此独立，因此两者样本数可能不同。

Metrics 的 LLM 页使用当前选择的时间窗口聚合相同口径，并在趋势图中按时间桶展示；无有效样本的时间桶保持空值，不按 `0` 处理。Share 性能表按时间窗口汇总各 Share，而不是复用卡片的最近 10 条限制。请求状态按 `usage_revision` 合并，旧的 pending 状态不能覆盖较新的 completed/interrupted 终态；模型替代成功率也只统计 `success` 和 `error` 终态请求。

### 运维事故与即时通知

Metrics 采集器持续判断 FD、CPU、内存、磁盘、SSH route 生命周期、数据库/EMFILE 新增错误和 LLM 错误率/限流。Client 离线不另做心跳算法，而是复用 Client 生命周期通知的可信 presence 状态机：确认离线、稳定恢复和离线后最终删除会在同一业务事务写入 `operator_alert_signal_outbox`，再由告警 worker 幂等写入 metrics 数据库。

Router 还会并发读取三路 HTTPS `Date` 响应,以两路仲裁和 RTT 中点估算主机时钟偏差。由于 Server 接受 ingress 最多慢 30 秒、最多快 5 秒,告警阈值也是非对称的:慢 15/25 秒进入 warning/critical,快 2/4 秒进入 warning/critical。Router 进程只监控,不持有 `CAP_SYS_TIME`;系统校时由独立的 systemd oneshot 与 timer 完成。完整部署、故障指纹和恢复步骤见 `docs/runbook-clock-skew-401.md`。

每个 fingerprint 同时最多一个未恢复事故，状态为 `firing`、`acknowledged`、`silenced` 或 `resolved`。新建、升级、提醒、恢复通知、静默到期和手动恢复都会记录 transition；通知 payload 在 transition 创建时冻结。确认、静默或更新的可通知 transition 会把尚未发送的旧投递置为不可重试的 `superseded`，避免恢复后再送达过期 firing 消息；曾收到高等级告警的渠道仍会收到对应恢复通知。投递使用 claim lease、指数退避和最多 12 次自动尝试，失败后进入 dead-letter，可在 Metrics 页面手动重新排队。`DELETE /v1/admin/metrics` 只清采样与旧 metrics event，不删除事故、投递或待处理 Client 信号。

告警投递、状态查询和测试 API 均以通用渠道 ID 工作，事故与 outbox 模型不依赖具体供应商；当前唯一注册的适配器是 Telegram，通过 Bot `sendMessage` 投递。Settings 页面可独立测试已注册渠道并显示最近真实投递/测试状态；Metrics 页面可确认、定时静默、恢复事故通知和重试失败投递。未来新增渠道只需增加配置、适配器和渠道注册，不需要重写事故状态机、投递存储或管理 API。

Share Market 与 Client Market 按产品和价格类型使用四项独立供应商准入策略：免费商品默认黑名单模式（默认开放），付费商品默认白名单模式（仅可信买家）。供应商可预先按买家邮箱建立信任，也可处理买家从具体商品发起的准入申请；批准付费申请时必须原子授予 USD 有限或无限信用额度。切换付费作用域到黑名单模式必须显式确认风险，且未知买家只有在供应商另行开启有限公共额度后才能租用付费商品；公共额度不能设为无限。

付费商品共用账户级后付费赊账：每项服务先享受 12 小时健康时长试用,之后只按 Router 观测到的健康服务秒数累计固定 USD 每日费用。同一买家和供应商共用一个 USD 余额；有限额度使用达到 80% 时向相关 Client 公开聊天室写入系统预警,用满后生成聚合账单并暂停相关服务。账单按 Settings 中的美元兑人民币汇率同时提供双币金额，默认 `1 USD = 7 CNY`；出账时冻结汇率和人民币金额，后续设置变更不改写历史账单，CNY 不形成独立账户。无限额度不自动出账,供应商可主动要求清账；买家也可主动清账,最后一项服务结束时剩余余额会自动出账。Router 不经手资金,付款声明仍需供应商确认到账；逾期声明或争议不会自行解除市场赊账限制。

Client 公开聊天室与 `installation.id` 一一对应,只为已验证 Owner 的 Client 建立；同一 Client 下的所有 Share 共用这一房间,不存在 Share 独立聊天室。历史消息公开可读,发送真人消息必须使用 Router 登录 Session;普通用户 API Token 不能发送。匿名访客的最近聊天室和已读游标只保存在当前浏览器,登录后会一次性合并到服务端用户记录。非 Owner 真人消息在同一聊天室内从第一条消息开始使用固定 60 秒窗口聚合,窗口内每条消息都完整写入同一封 Owner 邮件;Owner 自己的消息和系统消息不会触发聊天邮件。消息与邮件事件在同一业务库事务落库,后台使用固定 Resend 幂等键、claim lease、重试和 dead-letter。Client 被清理后聊天室转为公开只读归档并保留 60 天,同一 Client 在期限内恢复时沿用原房间。

Share Market、Client Market 与统一账务的关键事件通过持久化 outbox 写入对应 Client 公开聊天室。租用双方的完整邮箱、账单金额、收款方式与联系方式、付款 reference/note、凭证 URL、争议或回收原因以及安全的原始错误均公开展示；系统消息引用的同源收款图片随消息公开并在消息保留期内防止清理,未发布图片仍需 Owner 或账单买方身份。API Key、OAuth/Session token、Cookie、Authorization、密码、secret、私钥和 SSH/lease 凭据禁止进入 Market 源事件和聊天室 payload；后端在持久化前拒绝敏感字段与 query/fragment/userinfo 带凭据的 URL,并替换外部错误或备注中的凭据片段,前端渲染时再执行一次同类过滤。`PaymentMethod.token` 只允许表达 `USDT`/`USDC` 资产符号。验证码、安全通知、Client 注册/离线生命周期邮件和真人聊天提醒邮件仍保留,Market/Billing 业务事件本身不再发送交易邮件。

旧留言板、`/v1/board/*` API 及其 Telegram 推送配置已彻底移除；Client 公开聊天室是唯一的站内讨论渠道。

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
# {"ok":true,"database":{"mode":"local","available":true,...}}
```

控制台:`http://127.0.0.1/`

`/` 和 `/v1/dashboard` 默认公开可读,不需要登录。

dashboard 当前行为:

- 未登录时 share 表格中的 API key 默认脱敏
- owner 或活动 canonical `role=shareto` grant 中的邮箱登录后,可看到对应 share 的 API key 明文
- 页脚 `PAGE ONLINE` 右侧在 free plan 且 Resend 返回 `x-resend-daily-quota` 时,会显示 `RESEND USAGE xx%`
- Resend 用量由服务端每 10 分钟主动请求一次并缓存;若响应头不存在,则页脚只显示 `PAGE ONLINE`

邮件登录相关端点:

- `POST /v1/auth/email/request-code` 请求邮件验证码
- `POST /v1/auth/email/verify-code` 校验验证码并签发 access / refresh token
- `POST /v1/auth/session/refresh` 刷新会话
- `GET /v1/auth/session/me` 查询当前浏览器登录态

`GET /v1/public/map-points` 返回公开地图所需的点位数据,其中 `clients` 是按国家质心聚合后的地图点数组,每个点包含 `count`;`clientCount` 是符合条件的真实活跃 client 总数,两者可能不相等。

### systemd 部署示例

生产 unit 位于 `deploy/systemd/cc-switch-router.service`,时钟兜底 unit/timer 位于同一目录,NTP 源配置位于 `deploy/timesyncd/60-cc-switch-router.conf`。Router unit 明确排除改系统时间能力；只有短生命周期的 HTTPS 校时 oneshot 具有 `CAP_SYS_TIME`。安装命令和验收步骤见 `docs/runbook-clock-skew-401.md`。

```bash
sudo systemctl daemon-reload
sudo systemctl enable cc-switch-router
sudo systemctl start cc-switch-router
sudo systemctl status cc-switch-router
```

Router 收到 `SIGTERM` 后先停止 HTTP 接入并最多排空 30 秒,再关闭 SSH
listener。示例 unit 将 stdout/stderr 交给 journald,由系统日志策略负责轮转。

## 当前限制

**协议与功能**

- 仅实现 HTTP/WebSocket tunnel,不支持任意 TCP 转发
- 邮件验证码登录是基于服务端持久化 session 的 bearer token,不是 JWT。Dashboard/Market 验证码按邮箱、auth device 和用途隔离；Client Web 验证码按邮箱、Client installation 和用途隔离。校验必须提交发码时对应的认证来源 ID
- Resend 用量展示依赖官方响应头 `x-resend-daily-quota`;该 header 通常只对 free plan 返回,不返回时页脚不会显示用量

**Share 数据一致性**

- 设备私钥由 `cc-switch-server` 以本地文件方式保存(`server.json` 内的 `router.identity`),未接入系统安全存储
- share 用量同步为「事件驱动最终一致」,由 `cc-switch-server` 在创建、状态变更、用量变更、删除时异步上报
- `cc-switch-server` 端 share 同步已做短延迟批量聚合,降低高频请求噪音
- Share owner / `userGrants` 以 `cc-switch-server` 的 Contract v2 descriptor 为准，`cc-switch-router` 负责持久化、鉴权和 dashboard 脱敏控制；frozen schema 中的旧 ACL 列只写空值且不参与读取

**运行与清理**

- `cc-switch-router` 会定时清理超过保留期的历史 lease,以及状态为 `expired` / `deleted` 的陈旧 share 记录
- 当请求经 Cloudflare 代理进入时,free share 限流会基于可信的 `CF-Connecting-IP` 识别真实用户 IP;直连源站时会回退到 socket peer IP,防止伪造头绕过限制
- 后台任务在进程关停时被直接 abort,不做单独排空

**已知架构限制**

- 所有业务库访问(含只读)串行通过单个 `Mutex<Connection>`,这是当前主要的并发瓶颈
- 当前 schema 只有 fresh-install baseline,不支持导入或原地升级任何旧业务数据库
- Turso 模式只支持单活 Router 写实例,远端不可用时不接受离线写
- 默认用户 API token 以明文列存储以支持 UI 重复展示;数据库泄露等同于活跃 token 泄露
- 注册限流的内存 token bucket 无持久化,进程重启后短时间内尝试速率保护失效

详见 [ARCHITECTURE.md](ARCHITECTURE.md)。
