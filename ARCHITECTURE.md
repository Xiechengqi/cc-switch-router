# cc-switch-router 架构

本文档描述 `cc-switch-router` 的**当前实现现状**。协议层面的接口契约见 [PROTOCOL.md](PROTOCOL.md),部署与配置见 [README.md](README.md)。

> 本文替换了 2026-04 的同名设计提案。那份文档写于项目立项期,描述的是「建议改成…」的目标形态,且以 `cc-switch` Tauri 桌面版为客户端。实现已大幅偏离该提案,客户端也已换为 `cc-switch-server`。

---

## 0. 术语

`provider` 一词在本仓库有**两个不兼容含义**,文档中一律加限定词区分:

| 术语 | 含义 | 代码位置 |
|---|---|---|
| **Host Provider**(主机供给方) | Client Market 中出租服务器的用户 | `client_market_trade.rs:72` `provider_id` |
| **Upstream Provider**(上游供应商) | Share 绑定的 API 后端:claude / codex / gemini / kiro / cursor | `models.rs:1989` `ShareUpstreamProvider` |

与 Server 侧对齐的其余词汇:

| 术语 | 含义 |
|---|---|
| `installation` | 一个运行中的 Server 实例,Router 注册的基本单位 |
| `client tunnel` | Server 属主自己的管理端点隧道 |
| `share tunnel` | 对外提供额度共享的隧道,每个启用的 share 一条 |
| `share descriptor` | Server 同步给 Router 的 share 配置与运行时快照;一个 descriptor 可绑定 Claude/Codex/Gemini 中的 1 到 3 个 app |
| `account` | 绑定在 Upstream Provider 上的凭据(OAuth token 或 API key),存于 Server |
| `capacity pool` | 同一物理账号或 API key 的匿名容量标识;可跨多个独立 Share URL 复用，在凭据源变化时重派生 |
| `control_secret` | 注册时下发的对称 HMAC 密钥,用于 Router → Server 方向认证 |
| `ingress context` | Router 注入转发请求的签名身份上下文 |
| `pending share edit` | Router 侧排队的 share 变更,由 Server 拉取并 ack |

---

## 1. 系统定位

cc-switch-router 是 TokenSwitch 的**公共汇聚层**。它为 `cc-switch-server` 实例提供公网可达性，并在 Client + Router 边界内承载 Router 自有的 Share Market、Client Market 与中性的 Gateway 容量适配。旧独立 Token Market 不再是 Router 的运行时角色。

单进程同时承载三个职责:

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

核心依赖:`axum`、`russh`、`libsql`、`tokio`、`reqwest`。

**客户端**:`cc-switch-server` 是唯一客户端。`install-client.sh` 负责在远程主机上部署它。

**多区域**:仓库根部 `regions` 文件声明区域到域名的映射,当前为 `japan` / `singapore` / `hongkong` / `usa` 四个区域,经 `GET /v1/regions` 暴露(`src/api.rs:121, 2340`)。

---

## 2. Client + Router 的能力边界

两类内建市场共用同一套隧道、Share descriptor、准入和账户级后付费账务；它们都不依赖旧 `router_markets` 注册或 bearer session：

### ① Share Market —— 固定拼车位租用

Share Market 内建于 Router,不注册为外部 `router_markets`。Share owner 只能从 Router 的 Share Market 页面通过「添加 Share」选择自己当前 active、尚未挂售的 Share,候选项同时展示 subdomain、owner 和已绑定应用，并创建最多 20 个拼车位。每个拼车位独立配置用户 Token/并发限制、每日价格和服务期限。

- Token 限额留空表示不限额，此时周期固定归一为累计且 UI 不显示周期；设置限额后才选择累计不重置、每天、自然周、每 7 天、自然月或每 30 天。Token 重置周期与服务期限相互独立。
- 免费和付费拼车位都可设置 1–365 天固定服务期限或无固定期限。固定期限从买家确认租用成功时开始，绝对到期时间同时冻结到 Subscription 和 Server grant policy；授权延迟或账单暂停不会顺延，到期后任何状态都不得恢复服务。
- 每日价格留空时是免费拼车位；不要求信用额度、不进入账务系统。付费固定期限到期时会立即终止账务合约，并通过现有 revoke 流程安全回收授权；回收失败时座位不会提前恢复为可租。
- 准入按 `share/free`、`share/paid`、`client_host/free`、`client_host/paid` 四个作用域独立配置。免费默认黑名单，未知用户可先体验；付费默认白名单，需 Owner 明确允许并授予信用额度。
- 付费拼车位先授权服务,前 12 小时不计费。体验期结束后,Router 只按实际健康服务区间累计费用,未知或不可用时间不计费。
- 付费 Share 与 Client Host 共用按「买家 + 供应商」聚合的 USD 赊账账户。有限信用额度使用达到 80% 时向买卖双方预警,用满、任一方主动清账或最后一个服务结束时生成合并账单并暂停相关服务；无限额度只接受主动清账。美元兑人民币汇率由动态 Settings 管理并默认 `1:7`；未出账估算使用当前汇率，账单冻结出账时的汇率、人民币总额和明细，CNY 不参与记账。
- 用户按供应商收款资料线下付款并声明,供应商确认后恢复仍有效的服务；逾期会限制用户继续使用市场赊账。争议由 Router 管理员裁决,账单也可由管理员作废。
- 出账时会把供应商当时的收款方式和联系方式冻结到该账单,避免后续资料修改改变未结账单的付款依据或争议证据。
- 供应商可随时永久关闭某个买方的赊账关系；即使已有待处理账单,关闭意图也会立即锁定并终止服务,清账或作废后不会恢复,未来也禁止双方再次建立付费租约。
- 买家点击租用先经过交易确认，确认内容固定展示 Owner、服务、在线状态、USD 日费或免费报价、独立服务期限，以及付费服务的 12 小时健康时长试用和按供应商聚合账单语义；提交继续携带 `offerRevision` 防止确认后报价被替换。
- 租用后,Router 才通过 pending Share edit 在 Server 上创建 `routerShareMarket` 管理的 `shareto` entitlement。普通 Share 编辑不能修改或删除这类 entitlement。
- Owner 可强制回收、回收并拒绝该买家后续 Share 租用,或停止挂售。停止挂售只关闭空闲拼车位,不打断现有租约。
- **重新挂售**:停止挂售且该 Share 上已无活跃租约后,可再次通过「添加 Share」新建 listing。若仍有进行中的租约,「添加 Share」不可选中该 Share；也可在 Mine 的 closed listing 上「添加拼车位」以重新打开同一 listing。
- 普通 Share 的 `freeAccess` 是独立的公开免费策略：默认私有，开启后任意持有效 Router 用户 API Token 的已登录用户可调用，匿名仍拒绝。它与 Share Market listing/subscription 严格互斥；候选列表排除 Free Share，业务事务和数据库 trigger 双向阻止“公开免费 + 活跃市场 entitlement”，尚未应用的“开启 Free”控制面编辑也会阻止新建或重新打开 listing。

市场状态与审计由 `share_market_listings`、`share_market_seats`、`share_market_subscriptions`、`share_control_operations` 和 `share_market_events` 持久化；Seat 与 Subscription 都冻结 `service_duration_days`，Subscription 另存从租用成功时计算的绝对 `expires_at`，`activated_at` 只记录授权实际生效时间。统一准入由 `market_supplier_access_policies`、`market_counterparties`、产品规则、`market_access_requests`、私有/公共授信及事件表持久化，其中策略和规则主键均包含 `pricing_kind`。准入申请批准及管理页批量保存都使用 libSQL Immediate 事务，避免出现“已允许但未授信”或只保存部分买家的状态。统一账务由 `market_credit_accounts`、`market_service_contracts`、`market_service_intervals`、`market_accrual_entries`、`market_invoices`、`market_invoice_lines` 及付款、争议、限制、事件表持久化。

账户 UI 以 `/account/market-readiness` 作为供应商运营摘要，聚合收款资料、待准入数量、账务待办与四项准入策略；实际写操作仍分别归属收款信息、市场准入和市场账务页面。账务页以待处理、应付、应收、历史和管理员争议分区，不引入独立的前端账务状态。

Share 的 token/并发限制、请求 usage、用户 `usageRebase` 重基线、模型健康和容量池是 Share/Router 运行时能力，不等于外部 Token Market 的用户账本。普通 Share 的旧官方价格比例已退役；Share Market 报价只属于 listing/seat。Router 只保存本地 Share/seat entitlement、脱敏 observation 和服务账务；未来外部平台的下游用户、API key、token 售价、余额与 token settlement 必须留在平台自身。用户周期重基线由 Server 保存并通过 descriptor 下发，Router 不拥有编辑权。

### ② 中性 Gateway 容量适配

`/v1/gateways/register`、`/v1/gateway/*` 与 `/_gateway/proxy/*` 是未来跨 Router 容量消费者的基础入口。Gateway 使用 Ed25519 公钥、timestamp、nonce、原始 body SHA-256 和 scope；当前 self-reported owner email 仅用于本地审计，绝不参与授权。完整 tenant/seat/grant contract 尚未形成，而旧 `forSale=Yes + marketAccessMode=all` 已退役，因此当前 Gateway Share inventory/proxy 对普通 Share 整体 fail-closed。它是**适配层**而不是第三个交易面，也不能被描述为已完成的 Token Market。

### ③ Client Market —— 主机供给

Host Provider 贡献一台 Linux 服务器,Router 用**专用外发 Ed25519 provision key**(与入站 SSH host key 相互独立,`src/provision_ssh.rs:27`)登录,安装依赖并部署 Server,使其成为 Client + Router 容量与 Share 服务的供给节点。

主机状态机(`router_ssh_hosts.status`):

```
idle ──► locked ──► allocated ──► draining ──► idle

异常态:unreachable / abnormal / disabled / reserved
```

`reserved` 用于报价锁定期(`client_market_trade.rs:29`)。

**任务与清理对账**(`src/client_market.rs`):

| 场景 | 处理 |
|---|---|
| `draining` 停滞超 10 分钟 | 自动派发 cleanup job |
| `unreachable` 超 5 分钟 | SSH 探测;确认无安装痕迹则清除 DB 记录并复位为 idle |
| 进程重启导致 job 中断 | 启动时 `reconcile_interrupted_jobs` 以最多 4 并发重跑 |

这里的后台重试仅覆盖 Router 自身的首次开通 job 和显式 cleanup job。首次开通脚本只启动一次 Client 进程，安装的 systemd/OpenRC 服务不启用开机启动、失败重启或 respawn。开通完成后，Router 不会因 tunnel 离线而通过 provision SSH 启动或重启 `cc-switch-server`；只记录 `online`、`reconnecting`、`offline`、`disabled` 连接状态并继续心跳、告警和 UI 提示。Client 进程生命周期完全由 Client owner 管理。

**准入与支付均和 Share Market 共用统一机制**:免费 Host 使用独立的 `client_host/free` 作用域，默认黑名单；可配置 1–365 天固定期限或永久，期限在 Client provisioning 成功后才开始，到期复用安全 cleanup。付费 Host 使用默认白名单的 `client_host/paid` 作用域，还要求买家获得 USD 私有额度，或在该付费作用域切为黑名单后使用有限公共额度。付费 Host 以固定 USD 每日价格提供,先享受 12 小时健康服务时长试用,之后只按 Router 观测到的健康区间累计费用。同一买家和 Host Provider 下的 Host 与 Share 共用 USD 余额,按买家额度出账。Router 只记录链下付款声明,供应商独立核验到账后确认；只有确认到账或管理员作废账单才会解除对应逾期限制。Provider 租用自己的付费 Host 时按免费处理,不会形成自债务。

### Router 联邦 —— 横向扩展

未来外部容量消费者将以 Gateway 身份按 Router 分别注册，并通过版本化 grant 读取该 Router 的 Share capacity、查询已授权 headroom、反馈已授权 Share 的运行状态或使用签名 proxy。当前 grant contract 尚未形成，普通 Share 可见集为空并整体 fail-closed。observation 只允许保存 Gateway principal、已授权 Share/model、状态、延迟、token 数和地域；不保存下游用户/API key、USD 价格、余额或 settlement。Gateway 不跨 Router 共享 Router-local session。

| 表/入口 | 主体 | 认证/状态 |
|---|---|---|
| `router_gateways` / `/v1/gateways/register` | 单个 Router 上的 Gateway 公钥身份 | 公开基础注册；当前 owner email 为本地审计字段 |
| `/v1/gateway/*`、`/_gateway/proxy/*` | Gateway capacity adapter | Ed25519 signed headers (`x-cc-gateway-*`)、timestamp、nonce、body SHA-256、scope |
| `gateway_request_observations` / `capacity_request_observations` | 中性 observation 写入与兼容读取面 | 仅 Gateway source；不含旧 Market identity、价格或 settlement |

旧 `router_markets`、Market host/session/proxy 和 `/_market/proxy/*` 已从 active path 退役。Migration 19 创建并校验临时不可写 archive；migration 21 在再次校验后物理删除全部旧 live/archive 表，只保留不含身份的聚合 retirement receipt。

---

## 3. 隧道数据路径

```
浏览器 / CLI
  → Cloudflare
  → axum (:80) → proxy_handler                     src/proxy.rs:2141
  → subdomain_for_host()  剥离 tunnel_domain 后缀    src/proxy.rs:5746
  → ProxyRegistry.routes 查表
  → reqwest 请求 127.0.0.1:<临时端口>
  → io::copy_bidirectional ↔ SSH forwarded-tcpip    src/ssh.rs:491
  → cc-switch-server → 真·上游供应商
```

**路由表**是子域名到逻辑路由的映射:

```rust
routes: Arc<RwLock<HashMap<String, LogicalRoute>>>
```

`LogicalRoute`(`src/proxy.rs:106`)用三段式管理连接世代,支撑无损切换:

| 字段 | 作用 |
|---|---|
| `active: Option<RouteEntry>` | 当前生效的后端 |
| `candidates: BTreeMap<u64, RouteEntry>` | 已注册但未提升的连接,按世代号排列 |
| `draining: BTreeMap<u64, RouteEntry>` | 老连接排空中,上限 5 分钟 |

路由处于 `Reconnecting` 状态时,新请求经 `watch` channel 最多阻塞等待 3 秒再返回错误(`src/proxy.rs:1136`),而非立即 502。

响应体经 `bytes_stream()` 流式透传,并用 RAII guard 持有并发许可与流量记录直至流关闭(`src/proxy.rs:2953-2967`)。

> **已知行为**:排空窗口内新老两条 SSH 连接可同时为同一子域名转发流量。这是优雅轮换的设计取舍。

---

## 4. 认证与准入

### SSH 面

见 [PROTOCOL.md](PROTOCOL.md) 第 5 节。要点:仅一次性密码认证、用户名必须为 UUID、无 shell / exec / sftp、只放行 `tcpip_forward`。

### 分层限流

| 层 | 结构 | 存储 |
|---|---|---|
| 注册准入 | 全局 / 来源 / 公钥三级 TokenBucket | 内存(`registration_admission.rs:465`) |
| 新身份配额 | 10 分钟 / 小时 / 日 滑窗 | 业务 libSQL(重启不重置) |
| 未绑定 Owner 水位 | 默认 50000,达到后暂停新身份准入 | 业务 libSQL |
| 请求并发 | 6 个 `KeyedConcurrencyLimiter` | 内存(`proxy.rs:562-573`) |
| 认证滥用 | 10 分钟 10 次失败 → 封禁 1 小时 | 内存(`abuse.rs:6-8`) |
| 请求体体积 | 三档上限:普通 10 MB / 视频 32 MB / 图片 48 MB(可配置) | 内存缓冲上限(`proxy.rs` `proxy_request_body_limit()`) |

请求体体积不是速率限制,而是**内存缓冲天花板**:请求体经 `axum::body::to_bytes` 一次性读入内存,峰值内存 ≈ 单档上限 × 并发请求数。三档由 `CC_SWITCH_ROUTER_PROXY_{,MEDIA_,IMAGE_}REQUEST_BODY_LIMIT_MB` 配置(MB 为单位,改后需重启)。读取发生在 `try_acquire_share_permit` 之前,因此超限请求返回 413 且不消耗 Share 并发额度。

Router 转发到 Client 时,会在签名头旁再写一个**不参与签名**的 `x-cc-switch-ingress-body-limit`(十进制字节,值 = 该请求命中的档位),Client 取 `min(本地上限, 声明值)` 作为本次 ingress 的生效上限。因此:

- 该头不必签名——伪造只能把上限压低(伪造者自伤),抬不高 Client 的本地配置;`is_internal_share_context_header()` 还会剥离来自公网的同名头。
- 两端可独立升级:旧 Client 忽略该头、沿用自身硬编码值;旧 Router 不发该头,新 Client 回退到历史默认值(普通 2 MiB / 视频 32 MiB / 图片 48 MiB)。
- 新版 Client 的本地上限默认取 Router 允许的最大档位(64/256/256 MB),所以默认由 Router settings 决定实际天花板;卖家可在 `server.json` 的 `requestBodyLimits` 或 `CC_SWITCH_{,MEDIA_,IMAGE_}REQUEST_BODY_LIMIT_MB` 中主动收紧。

并发限流键位:`share_id`、`share_id:app`、`share_id:app:email`、用户 IP(免费档)、图片任务、市场邮箱。

> **已知限制**:内存 TokenBucket 无持久化,进程重启即清零。业务库侧的新身份配额能兜住真正的身份创建,但尝试洪水本身在重启瞬间不受限。

### 真实客户端 IP

`src/cf.rs` **不调用任何 Cloudflare API**,它只硬编码 Cloudflare 的 IPv4/IPv6 网段(`cf.rs:15-42`),用于判断 TCP peer 是否为 CF 边缘节点。

- peer 是 CF → 信任 `CF-Connecting-IP` / `CF-IPCountry` / `CF-ASN`
- peer 非 CF → 回退 socket peer IP

这防止伪造头绕过免费档限流(`client_meta.rs:11`、`proxy.rs:4122`)。

---

## 5. 数据层

业务库与 metrics 库分离。最终 schema 当前有 116 张非 SQLite 内部表（包含 `schema_migrations`）；数量由 fresh-schema 测试固定。

**数据库模式**:

- `local`:独立本地 libSQL 文件,WAL 模式。
- `turso`:Turso Cloud Embedded Replica,本地读、远端委派写,周期拉取远端 frame。未启用 offline writes,远端不可用时写操作失败并将健康状态置为 unavailable。
- metrics 始终通过独立本地 `libsql-rusqlite` 文件存储,不进入 Turso。除采样表外,持久化事故、transition、投递 outbox/attempt、渠道测试和 source-event 去重也位于该文件；清空 metrics 采样不会清理这些事故表。

**连接模型**:业务读写保留单个 `Arc<Mutex<Connection>>`,外键开启、`busy_timeout = 5000ms`;local 模式额外启用 WAL。同步 facade 在专用 Tokio runtime 执行 libSQL async I/O,从而保持现有 store 的同步 SQL 边界。Turso 周期同步通过独立的 database handle 运行,健康快照通过共享原子状态读取,两者都不获取业务连接 mutex。

**Schema 策略**:仅支持全新环境。空库首次启动在 `BEGIN IMMEDIATE` 事务中安装 `schema/0001_baseline.sql`,随后写入版本与 SHA-256 checksum。非空且没有迁移元数据的旧库、未知版本和 checksum 不匹配都会拒绝启动,不执行历史探测、补列或数据迁移。

**baseline 不可再修改**:已部署的库都记录了 `schema/0001_baseline.sql` 的 SHA-256,改动该文件会让所有既有库以 `database migration 1 checksum mismatch` 拒绝启动,同时使自升级在 `check-db` 自检阶段静默失败。任何后续 schema 变更只能新增 `schema/00NN_*.sql`,`src/schema.rs` 的 `baseline_checksum_stays_frozen` 测试钉死了该文件的 checksum。

**Turso 运行约束**:只支持单个可写 Router 实例,不包含 leader election 或多写协调。`/v1/healthz` 暴露模式、可用状态、最近同步/失败时间、连续失败次数与同步 frame 数；远端故障时返回 503,但不向调用方暴露底层连接错误或 Token。

### 表分组

| 域 | 代表表 |
|---|---|
| 身份与隧道 | `installations`、`leases`、`tunnel_route_heads`、`shares`、`installation_client_tunnels` |
| 计量 | `share_request_logs`、`gateway_request_observations`、`capacity_request_observations`、`llm_request_metrics`、`image_generation_*` |
| 健康 | `share_health_checks`、`installation_health_checks`、`share_model_health_state` |
| 统一市场账务 | `supplier_billing_profiles`、`market_credit_accounts`、`market_service_contracts`、`market_service_intervals`、`market_accrual_entries`、`market_invoices`、`market_invoice_lines`、`market_payment_*`、`market_billing_*`、`market_credit_restrictions` |
| 主机市场 | `router_ssh_hosts`、`client_market_subscriptions`、`account_payment_*` |
| 联邦与市场 | `router_gateways`、`share_market_listings`、`share_market_seats`、`share_market_subscriptions`；旧 registry/live/archive 表均已由 migration 21 物理删除 |
| 通知 | `installation_notification_state`、`client_notification_events`、`client_notification_runtime`、`notification_deliveries`、`notification_delivery_items`、`notification_delivery_attempts`、`user_notification_channels`、`telegram_bot_runtime`、`telegram_bind_tokens`、`telegram_inbound_updates`、`telegram_poll_cursors` |
| 运维告警信号 | `operator_alert_signal_outbox` |
| 聊天 | `chat_rooms`、`chat_messages`、`chat_visits`、`share_presence_state`、`client_chat_system_outbox`、`chat_public_payment_assets`、`chat_rate_limit`、`chat_email_events`、`chat_email_deliveries`、`chat_email_delivery_items` |
| 认证 | `users`（含用量卡片公开开关）、`user_sessions`、`user_api_tokens`、`email_login_challenges` |

### 账户用量（Provider / Consumer）

账户页提供双视角用量（只计 model + token，不算 cost）：

- **Provider**：`owner_email` 下多 installation / share 的被消耗量 → `GET /v1/me/usage/provider`
- **Consumer**：`user_email` 跨 share 的调用量 → `GET /v1/me/usage/consumer`
- **用量卡片设置**：`GET/PATCH /v1/me/usage-card`；公开统计默认开启，卡片身份直接使用账号邮箱，公开 URL 使用稳定的用户 UUID
- **公开 SVG**：`GET /v1/public/embed/global.svg`（全站）、`GET /v1/public/embed/usage/:user_id`（可关闭的个人卡片）；默认 `24h`、浅色主题
- 数据来自现有 `share_request_logs` / Gateway-only `capacity_request_observations` 短窗口聚合（24h / 7d / 30d）；migration 21 只把能关联已知 Share 且具备用户身份的旧 observation 最小化迁入 `share_request_logs`，原 `market_request_logs` 与 archive 已物理删除

### 已知限制

- **单连接全局锁**:所有业务 store 方法(含只读)都要 `self.conn.lock().await`。local 模式的 WAL 不缓解进程内这把 tokio Mutex 的串行化；Turso 委派写也经过该锁。周期 replica sync 和健康快照不经过该锁。这仍是当前最确定的扩展性瓶颈。
- **单活 Turso 写实例**:当前不支持多个 Router 同时写同一 Turso 数据库；云端不可用时写请求直接失败,没有本地补写队列。
- **`user_api_tokens.token_plaintext`**:默认 token 以明文存储,以支持在 UI 中重复展示 API key(`store.rs:12756, 23472`)。同表已有 `token_hash`。DB 泄露即等于活跃 token 泄露。
- 健康检查类表使用 AUTOINCREMENT 追加,依赖 `cleanup_expired_data` 清理而非 schema 层 TTL。

### Client 公开聊天室与市场事件

聊天室身份只使用 `installation.id`。每个已验证 Owner 的 installation 最多有一个 active 房间,该 Client 下所有 Share、Share Market 租约、Client Market 租约和统一账务事件都写入同一房间；系统不创建 Share 房间或 Share 成员期。

业务事务先在同一 libSQL 事务内写入 `client_chat_system_outbox`,后台再物化为 `author_kind=system`、`message_kind=market_event` 的不可删除消息。`source_kind + source_event_id + installation_id` 提供幂等键；失败事件指数退避,达到上限进入 dead-letter,不会阻塞后续事件。Owner、Provider、租客和事件 actor 会自动写入 `chat_visits`,因此可在各自入口看到房间与未读数。

市场系统消息公开完整交易上下文,包括双方邮箱、金额、收款资料、付款凭证、reference/note、争议或回收原因和不含凭据的原始错误。物化系统消息时,仅当同源收款图片属于 payload 中的 Owner/Provider/Supplier,才写入 `chat_public_payment_assets`；该映射随消息级联删除并阻止资料更新提前清理图片。后端禁止 API Key、OAuth/Session token、Cookie、Authorization、密码、secret、私钥、SSH/lease 凭据和带凭据 query/fragment 的 URL 进入 outbox；仅 `kind=crypto` 的 `USDT`/`USDC` 收款资产符号可使用 `token` 字段。前端渲染前再次过滤。系统消息不创建 `chat_email_events`,只有真人访客消息可触发 Owner 聊天提醒邮件。

---

## 6. 控制平面

Router 需要把 dashboard 上的 share 配置变更推送到 Server。两条路径:

**同步路径**(首选,`src/ctl_client.rs`):复用已建立的反向隧道,直接调用 Server 的 `/_ctl/apply_share_settings`,HMAC-SHA256 认证。延迟从「一个轮询周期」降到「一个 RTT」。

**异步路径**(降级):写入 pending-edit 队列,Server 经 `POST /v1/shares/pending-edits` 拉取、`POST /v1/shares/edit-ack` 确认,并可通过 `GET /v1/shares/edit-events`(Ed25519 签名的 SSE 流)获得即时唤醒。

降级触发条件见 [PROTOCOL.md](PROTOCOL.md) 第 7 节。

Router 从不改写 Server 返回的 descriptor,只做校验;若客户端只部分应用补丁,Router 拒绝落库而非静默持久化(`store::validate_returned_share_against_patch`)。

---

## 7. 后台任务

`main.rs` 在监听器绑定后拉起以下 `tokio::spawn` 任务:

| 任务 | 周期 | 职责 |
|---|---|---|
| `cleanup_task` | 300s | 过期数据清理 + 市场主机对账 |
| `probe_task` | 30s | 路由健康探测 |
| `runtime_task` | 10min | share 运行时快照刷新 |
| `resend_usage_task` | 10min | Resend 配额轮询 |
| `metrics_task` | — | 指标采集 |
| `alerting_task` | 2s | Client 信号入库、静默到期、已注册 IM 渠道投递与重试 |
| `notification_task` | 5s | 离线/恢复通知 outbox(邮件 + Telegram) |
| `telegram bot` | 长轮询 | 用户通知 Bot 的入站半边:消费 `/start` 深链完成账号绑定 |
| `chat_notification_task` | — | 聊天邮件投递 |
| `client_market_trade_task` | 20s | Client Market 报价、免费期限、释放与清理状态对账 |
| `share_market_task` | 5s | managed grant、免费期限与 Share 租约状态对账 |
| `market_billing_task` | 5s | 健康时长计费、阈值出账、最终账单、逾期限制和控制动作重试 |
| `ip_blacklist_log_task` | 600s | 黑名单统计落日志 |

**关停**:收到 SIGTERM 后先停 HTTP 接入并排空最多 30 秒,再关 SSH listener(5 秒)。

> **已知行为**:后台任务在关停时被 `.abort()`(`main.rs:492-500`),持有 DB 锁或处于事务中途的任务会被硬杀。

---

## 8. 通知系统

采用 outbox 模式,三阶段循环(`src/notifications.rs`):

1. **reconcile** —— 扫描在线状态,产出离线/恢复事件;推进持久化 baseline,使停用期间的事件不会补发
2. **aggregate** —— 同收件人事件在窗口内(默认 60s)合并为一个批次;风暴检测触发时进一步合并为 digest,并按用户启用的渠道集合扇出独立投递
3. **deliver** —— worker 以 90 秒租约认领批次,按批次自身的 `channel` 经 Resend 或 Telegram Bot API 发送

可靠性设计:

- 聚合时冻结渠道载荷和稳定幂等键,重试不会重新渲染正文
- 12 次尝试后进入死信
- 可重试状态码:408、425、429、5xx,以及带 `concurrent_idempotent_requests` 的 409
- 单收件人与全局双层小时配额按真实发送 attempt 统计,offline 与 registration 两条 lane 额度互不占用

### 8.1 通知渠道抽象

渠道 ID 使用可扩展的小写字符串(`src/notification_channels.rs`),用户偏好、outbox、attempt 和 transport registry 均不依赖组合枚举。`user_notification_channels` 每个用户/渠道一行,`enabled` 与绑定 `state` 分离；目标、provider identity 和 revision 会冻结到 `notification_deliveries`。外部调用开始前再次验证 revision,因此关闭、解绑、换绑或 Bot 身份变化会取消尚未开始的旧目标。`target_revision = 0` 只用于没有账户渠道归属的强制注册邮件与兜底邮件。

- **通用 outbox**:`notification_deliveries` 保存具体渠道、脱敏归属、冻结 payload 与结构化失败信息；`notification_delivery_items` 以 event/recipient/channel 唯一,同一事件可以在多个渠道各投递一次。
- **attempt 级额度**:claim 时先写 `reserved`,provider 调用前改为 `started`,结果最终进入 `sent`、`retry`、`failed` 或 `cancelled`。小时额度统计 started attempt 与未过期 reservation；过期 attempt 会先终结再重新认领,不会永久占住 live-attempt 唯一索引。
- **回落而非丢弃**:目标集合为空、Bot 未启用或 Bot 身份不可用时至少生成 Email。Registration lane 固定走邮件,首次登录口令提示不进入第三方 IM。
- **并发关闭语义**:禁用渠道或解绑会取消 pending/retry 及未开始的 reservation；已经进入 provider 调用的 started attempt 可完成,避免在未知发送结果下重复投递。
- **不可达处理**:Telegram 明确返回 chat 不可达时,事务会校验 delivery 归属、Bot identity 与 chat target,结束当前 attempt,只使匹配绑定失效,并释放原事件生成一次邮件回落。

绑定链路:`POST /v1/me/notifications/telegram/bind-link` 只持久化 128 位 token 的 SHA-256,返回基于已验证 Bot username 的深链。已撤销 token 继续保留到清理期,所以反复申请/撤销不能绕过用户与 IP 小时限额。polling 与 webhook 都先以 `(bot_id, update_id)` 幂等写入 `telegram_inbound_updates`;polling cursor 与整批 insert 在同一事务推进,handler 使用租约重试并且不会因单条失败丢弃后续输入。Bot token 热更新后先调用 `getMe`:Bot ID 不变时保留绑定并恢复对应阻断投递,Bot ID 变化时撤销旧链接、使旧绑定失效并启用邮件回落。该 Bot 与下节的运维告警 Telegram adapter 配置和状态相互独立。

### 8.2 运维事故与 IM 渠道

`src/alerting/` 使用 metrics SQLite 保存 `alert_incidents`、`alert_transitions`、`alert_deliveries`、`alert_delivery_attempts`、`alert_channel_checks` 和 `alert_source_events`。Fingerprint 保证同一条件同时只有一个活跃事故；Metrics 条件按完整观测集 reconcile，Client presence 则通过业务库 `operator_alert_signal_outbox` 跨库传递，source event ID 同时在两侧去重。

事故状态为 `firing`、`acknowledged`、`silenced`、`resolved`。确认与静默不伪造恢复；严重级别升级会重新唤醒已确认事故，静默到期仍异常时自动回到 firing，真实恢复始终产生 recovery transition。每次可通知 transition 在同一 metrics SQLite 事务内冻结渠道 payload 并创建 delivery；同一事故尚未发送的旧 delivery 会先进入不可重试的 `superseded`，且曾接收高等级 firing 的当前启用渠道不会因事故降级而漏掉 recovery。worker 使用 60 秒 claim lease、带稳定 jitter 的指数退避和 12 次自动尝试；attempt number 防止过期 worker 覆盖已被重新认领的投递。

渠道适配器通过稳定的字符串 ID 注册，投递 policy、outbox、渠道状态和管理 API 均不依赖具体供应商。当前唯一注册的 Telegram adapter 调用 Bot `sendMessage`；未来渠道可在不改动事故状态机和存储模型的前提下加入。所有渠道错误在持久化和 API 返回前截断并清除换行或 Token，Secret Settings 只返回 `hasValue`。

---

## 9. 前端

Next.js 静态导出(`output: "export"`),`build.rs` 遍历 `frontend/out/` 生成 `include_bytes!` 匹配表编译进二进制,由 `/*path` catch-all 提供服务。单文件部署,无外部资源依赖。

- 无外部状态库,纯 React Context
- i18n 覆盖 `en` / `zh-CN`
- **Settings 控制面**(`/settings/`):受管环境变量按 7 个稳定配置域组织。后端 schema 是字段类型、约束、依赖、风险和重启边界的唯一来源，前端 i18n catalog 只覆盖文案。`GET /v1/admin/settings` 在一个快照中返回 schema、持久化值、进程有效值、来源和 SHA-256 revision；validate/PATCH 都要求 `expectedRevision`。进程启动前已有的环境变量被标记为只读 override，Secret 只暴露 `hasValue`。静态字段以启动快照计算 durable pending-restart，动态字段保存后直接更新 `DynamicSettings`；快照从当前 `DynamicSettings` 反向生成热更新字段的运行值，因此手工修改 `.env` 也不会被误报为已经生效
- **持久化边界**:`PATCH /v1/admin/settings` 在 `DynamicSettings` 写锁内完成 revision 校验、整组关系校验、`.env.new` 写入与 fsync、旧文件备份、原子 rename、目录 fsync，再发布动态快照。通知 lifecycle 同步失败会回写旧 `.env`。地图和公告使用独立 revision，前端在 409 时加载最新版本并保留用户草稿供复核
- **Operations 控制面**(`/operations/`):版本/服务操作、Router 日志、通知投递历史和 admin audit 从 Settings 中独立出来，避免配置编辑与即时运维动作混在同一导航和保存状态机中
- **配置契约审计**:`cargo test admin::settings` 覆盖后端 schema、来源、关系和文件权限；`npm run audit:settings-i18n` 保证所有字段与分组都有中英文文案；`npm run audit:settings-contract` 保证 Rust schema、默认 `.env` 和前端字段 catalog 精确一致，且旧 Settings API 不会回流
- **账户 → 通知设置**(`/account/notifications`):邮件与 Telegram 使用独立开关,至少启用一个渠道；Telegram 未绑定或 Bot 未就绪时不可开启,服务端执行相同约束。绑定按钮在 `await` 前先 `window.open("about:blank")` 保留用户手势,失败则回落为可复制深链与 `/start <token>`。绑定完成发生在 Telegram 侧,页面以 3 秒间隔轮询 `GET /v1/me/notifications`,并以 `verifiedAt` 变化识别重新绑定而不是误用旧绑定状态；5 分钟后自动停止
- 设计 token 以 `--router-*` CSS 自定义属性承载,dark mode 经 `.dark` class 整体切换
- **Router 自己的 Web 终端**:xterm.js ↔ WebSocket ↔ `portable-pty` 起 `ssh` 子进程(非 russh client),一次性 ticket + 每用户 2 会话上限 + 空闲/硬超时。**仅 Host Provider 本人可开**,租客与 Router 管理员均不可
- **Client 自带的 Web 终端**(另一条链路):Clients 页条目上的「终端」按钮,与「控制台」共用同一套 iframe 窗口管理器(`components/dashboard/client-console/`)。它不经过 Router 的 PTY,而是把 client web URL 加上 `?view=terminal&embed=1`(`lib/client-web-view.ts`)后在 iframe 中打开,请求经 client tunnel 转发到 Server 的 `/web-api/terminal/*`,鉴权由 Server 自身的 Web 登录态负责。`embed=1` 让 Server 前端隐藏自己的页头、状态条和「结束会话」按钮,只渲染 xterm 画布——窗口标题栏和关闭按钮由 Router 这侧提供,不重复一遍。窗口按 `clientId + kind` 去重,因此同一 client 的控制台与终端各占一个窗口

---

## 10. 自升级

`src/admin/upgrade.rs` 实现进程自替换:

1. 同目录暂存临时文件(避开跨文件系统 rename 的 EXDEV)
2. 从 GitHub latest release 下载(180s 超时)
3. `chmod +x`,以 `--help` 冒烟自检(5s)并用新二进制跑 `check-db`(30s)确认它能接受当前数据库
4. SHA-256 比对新旧二进制
5. 原子 `rename(2)` 交换,旧二进制留 `.bak`
6. 探测服务管理器(systemd 或 nohup)
7. `setsid -f` 派生子进程延时重启

进度经 broadcast channel 以 SSE 流式推送至前端。任一步失败都在交换二进制之前 `return Err`,进程继续运行旧版本;前端只在收到 `status=success` 的 done 事件后才轮询健康并刷新页面,失败时保留日志弹窗并把最后一条 error 显示为告警。
