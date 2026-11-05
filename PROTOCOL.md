# Router ↔ cc-switch-server 协议契约

本文档记录 `cc-switch-router`(以下简称 Router)与 `cc-switch-server`(以下简称 Server)之间的接口契约。

Server 是唯一可以注册为 **Client installation**、建立隧道并出现在 Client 监控中的程序。Dashboard 浏览器和公开 Share Web 使用独立的 **auth device** 身份,不会创建 Client。Router 内建 Share/Client Market 与未来外部容量平台通过独立的 Gateway 身份接入；它们不是 Client installation。

本文所有断言均标注 Router 侧源码位置(`file:line`),便于与实现对账。

---

## 1. 协议常量

| 常量 | 值 | 出处 |
|---|---|---|
| `PROTOCOL_EPOCH` | `namespace-flat-1` | `src/namespace.rs:1` |
| Client 注册动作 | `register_installation` | `src/store.rs::verify_registration_signature` |
| Auth device 注册动作 | `register_auth_device` | `src/store.rs::verify_auth_device_registration_signature` |
| Ingress 签名域 | `cc-switch-router-ingress-v1` | `src/ingress_context.rs:11` |

Epoch 参与所有 Ed25519 签名的规范串。两侧 epoch 不一致时**硬失败**,不做协商降级。注册协议没有独立 proof 版本字段或其他版本分支,当前规范串就是唯一格式。

---

## 2. 注册:两类 Ed25519 身份

### 2.1 Client installation

`POST /v1/installations/register`

Server 首次启动时生成 Ed25519 密钥对,公钥随注册请求上送。请求体字段见 `RegisterInstallationRequest`(`src/models.rs:118-130`):
`protocolEpoch`、`publicKey`、`platform`、`appVersion`、`instanceNonce`、`timestampMs`、`signature`。所有字段必填,未知字段会被拒绝。

**签名规范串**:

```
{PROTOCOL_EPOCH}\nregister_installation\n{public_key}\n{platform}\n{app_version}\n{instance_nonce}\n{timestamp_ms}
```

公钥与签名均为标准 base64。注册时 `installation_id` 尚不存在,故不入签名串;后续所有签名请求改用第 3 节的通用规范串。

**响应**(`RegisterInstallationResponse`,`src/models.rs:134-142`):

| 字段 | 说明 |
|---|---|
| `installationId` | Router 分配的实例 ID,后续所有请求的身份标识 |
| `controlSecret` | **对称** HMAC 密钥,与 Ed25519 密钥对相互独立。Server 必须持久化,并用它校验 Router 发来的控制平面调用与 ingress 身份头 |

> `control_secret` 是 Router → Server 方向的认证凭据;Ed25519 私钥是 Server → Router 方向的认证凭据。两者用途不可混用。

注册成功只建立 installation 控制身份,不会立即形成可见 Client。Server 还必须完成 owner 验证、启用同 owner 的 Client tunnel,并签名上报 setup 完成;Router 随后写入 `client_activated_at`。Dashboard、Metrics、Client 日志入口、Client chat、公开地图和 Share 清单只读取已经激活的 Client。在线状态只由 `/v1/installations/heartbeat` 更新,其他签名控制请求只更新一般活动时间。

### 2.2 Auth device

`POST /v1/auth/devices/register`

Dashboard 和公开 Share Web 生成独立 Ed25519 密钥对并注册 auth device。请求字段为:
`protocolEpoch`、`publicKey`、`kind`、`platform`、`appVersion`、`instanceNonce`、`timestampMs`、`signature`;`kind` 只允许 `browser` 或 `service`。

**签名规范串**:

```
{PROTOCOL_EPOCH}\nregister_auth_device\n{public_key}\n{kind}\n{platform}\n{app_version}\n{instance_nonce}\n{timestamp_ms}
```

响应只返回 `authDeviceId`。Auth device 没有 `controlSecret`、隧道、Share、Client presence 或 Client 可见性语义,其记录位于 `auth_devices`,不会写入 `installations`。

### 准入限流

两类注册都受尝试速率保护,新 installation 与新 auth device 使用独立的持久化额度事件。触发时返回 `429` 并携带 `Retry-After`。使用已有公钥恢复同类身份仍受尝试速率保护,但不消耗新身份额度。

---

## 3. 通用签名请求

注册之后的 installation 和 auth device 签名请求统一使用以下规范串:

```
{PROTOCOL_EPOCH}\n{identity_id}\n{action}\n{payload_json}\n{timestamp_ms}\n{nonce}
```

- Client installation 的 `identity_id` 是 `installationId`;auth device 的 `identity_id` 是 `authDeviceId`
- `payload_json` 的表示由 action 契约定义。普通结构化 action 使用契约声明的紧凑 JSON；下文明确标记为 raw-signed 的字段必须字节级等于请求体中的 JSON 原文
- 任何包含 `ShareDescriptor` 的 raw-signed action 都不得为验签把 descriptor 解开后再序列化；Server 必须只序列化一次被签名字段，并把同一份 JSON 原文嵌入请求
- 业务解析失败(未知字段、类型错误)是 4xx 解码/校验,不是 401
- `action` 为动作名,例如 `installation_setup_completed_v1`
- 签名为 Ed25519 签名的标准 base64
- installation nonce 由 `request_nonces` 拦截;auth device nonce 由 `auth_device_nonces` 拦截

### 3.1 Dashboard 邮箱登录 challenge

Dashboard 浏览器通过 `POST /v1/auth/devices/register` 建立 `kind=browser` 的 auth device。该身份是验证码 challenge 的设备边界,必须满足以下约束：

- 同一浏览器 profile 的并发初始化必须收敛到同一组 `authDeviceId`、公钥和私钥。单标签页使用 single-flight；支持 Web Locks API 时,跨标签页以 auth-device identity lock 串行初始化,并在持锁后重新读取持久化身份。身份失效后的自动替换也在同一把锁内执行,且只替换发起失败请求的旧 ID。
- `POST /v1/auth/email/request-code` 使用 action `auth_request_code` 和 `{ email, purpose: "login" }` 签名。前端必须保存该次请求实际使用的完整 auth device 身份快照。
- challenge 的逻辑作用域是 `email_normalized + auth_source_kind + auth_source_id + purpose`。同一邮箱可在多个 auth device 或 Client Web 上同时持有有效 challenge；重发只消费同一作用域的旧 challenge,不得影响其他设备。
- 同一邮箱和认证来源的并发发码在 Router 进程内串行执行,后到请求必须在前一请求落盘后重新经过冷却检查；等待邮件供应商时不得持有业务数据库连接锁。
- challenge 的 `created_at`、过期时间和重发冷却从邮件供应商确认发送成功后开始计算，发信耗时不占用验证码有效期。
- `POST /v1/auth/email/verify-code` 必须提交发送 challenge 时保存的 `authDeviceId`,不得在校验时重新解析可能已变化的浏览器身份。
- 正确验证码的消费、用户 upsert、Session 创建和默认 API Token 创建位于同一个 libSQL `IMMEDIATE` 事务；任一写入失败时全部回滚。错误验证码的 `attempt_count` 独立提交。
- 客户端只接收通用安全错误。Router 日志仅记录 `expired`、`consumed`、`source_mismatch`、`not_found`、`invalid_code` 或 `attempt_limit` 原因码，不记录邮箱、验证码或验证码 hash。

Client Web 邮箱登录使用 `/v1/client-web/auth/email/request-code` 和 `/v1/client-web/auth/email/verify-code`,由 Server 使用自己的 installation 私钥签名,因此 challenge 来源是 `client_installation`。Session 创建后只由 access/refresh token 标识；refresh 请求只提交 `refreshToken`,不会把 Session 生命周期绑到来源记录是否仍存在。

### 3.2 Router 可选密码登录

`POST /v1/auth/password/login` 接收 `email`、`password`、`authDeviceId`、`timestampMs`、`nonce`、`signature`，按 action `auth_password_login` 签名。payload 为规范化小写邮箱和原样密码的紧凑 JSON，字段顺序固定为 `{"email":"...","password":"..."}`。密码不能 trim。此 nonce 使用独立 action 持久化；签名时钟窗口沿用通用请求契约。请求体最大 4096 字节。

成功返回与验证码验证相同的用户和 Session token 字段（不创建／返回 API Token），并增加 `password: { hasPassword, version, canResetWithoutPassword, loginEventId }`。`session/me`、验证码验证和 refresh 也返回当前用户自己的该状态。未知用户和未设密码返回相同的 `401 password_unavailable`；已设密码但密码错误、账号禁用或邮箱未验证返回 `401 password_invalid`。

`PUT /v1/auth/password` 接收 `newPassword`、可选 `currentPassword` 和 `expectedVersion`。只接受当前有效 Session 显式 Bearer，校验浏览器 Origin，不允许 Cookie／API Token／开发模式替代。最近真实邮箱验证在 10 分钟内时无需旧密码，否则必须提供旧密码；首次设置必须有最近邮箱证明。旧密码错误和证明不足分别为 `403 password_current_invalid`／`403 password_email_required`，不当作全局 Session 401。并发凭据变更为 `409 password_changed`。尝试过量为 `429`，计算繁忙为 `503 password_busy`。

凭据更新原子提升用户 `auth_version` 并撤销全部旧 Session；网页更新返回替换 Session。CLI 更新不返回用户 Session。refresh 继承原认证方式、邮箱验证时间和 login event，不延长邮箱证明，也不能通过 rotation grace 恢复改密前的会话。历史 Session 的认证方式为 `legacy`，邮箱证明为空。CLI、限流与运维见 [密码登录文档](docs/password-authentication.md)。

### 2.3 Gateway（中性容量接入身份）

Gateway 不是 Client，也不是旧 Token Market 注册。它是一个外部容量消费者在**单个 Router**上的 Ed25519 公钥身份。当前注册接口仍是公开的基础注册；`ownerEmail` 是调用方自报、未验证的本地审计元数据，不参与 Share 可见性、proxy、headroom、feedback 或 observation 授权，也不会创建下游用户、API key、余额或 token 账本：

`POST /v1/gateways/register`

请求字段：`ownerEmail`、`displayName`、`publicKey`（Ed25519 公钥，标准 base64），以及可选的 `publicBaseUrl`、`appVersion`。同一公钥重复注册会更新该 Gateway 的显示信息并复用 `gatewayId`；当前默认授予固定 scope，撤销、轮换和 tenant 绑定尚未形成版本化外部 contract。由于还没有中性的 tenant/seat grant，且旧 `forSale=Yes + marketAccessMode=all` 已退役，Gateway 当前看不到任何普通 Share；inventory、proxy、headroom、feedback 和 observation 都必须 fail-closed，不能用自报邮箱、`freeAccess` 或 email ShareTo 绕过。

除注册外，Gateway 请求必须携带以下 headers：

| Header | 说明 |
|---|---|
| `x-cc-gateway-id` | Router 返回的 Gateway ID |
| `x-cc-gateway-timestamp-ms` | Unix 毫秒时间戳 |
| `x-cc-gateway-nonce` | 单次随机 nonce；Router 记录并拒绝重复值 |
| `x-cc-gateway-signature` | Ed25519 签名，标准 base64 |

当前签名规范串**准确为**（不含 HTTP method、path 或 protocol epoch）：

```
{gateway_id}\n{action}\n{body_sha256_hex}\n{timestamp_ms}\n{nonce}
```

`body_sha256_hex` 是实际请求体原始字节的 SHA-256 小写十六进制；无 body 的 GET 使用空字节串 hash；JSON 请求使用 Router/调用方序列化后的实际字节。Router 当前接受约 ±60 秒时间偏差，nonce 保留约 10 分钟。控制平面 `control_secret` 的 HMAC 规范（第 7 节）与 Gateway Ed25519 签名互不相同。

当前保留的 Gateway 端点：

| 方法 | 路径 | 必需 scope / action | 说明 |
|---|---|---|---|
| `GET` | `/v1/gateway/shares` | `gateway:shares:read` / `gateway:shares:read` | 返回本 Router 可见的 Share capacity 与脱敏运行信号；wire 使用 opaque `shareName`，不包含 Share/installation owner 或 Provider account email |
| `POST` | `/v1/gateway/shares/headroom` | `gateway:shares:read` / `gateway:shares:headroom` | 只查询该 Gateway 当前可见 Share 的并发余量；混入越权 ID 整批拒绝 |
| `POST` | `/v1/gateway/shares/feedback` | `gateway:feedback:write` / `gateway:shares:feedback` | 只对该 Gateway 当前可见的 Share 上报限流/配额反馈 |
| `POST` | `/v1/gateway/request-logs/batch` | `gateway:request_logs:write` / `gateway:request_logs:batch` | 幂等写入脱敏 Gateway observation（最多 500 条/批）；带 Share ID 的记录必须属于当前可见集 |
| `ANY` | `/_gateway/proxy/:share_id/*path` | `gateway:proxy:use` / `gateway:proxy` | 经过 Share ACL、App 路径和并发检查后转发 |

Gateway observation 的 active payload 只接收 request ID、Router/Share/model、请求 agent、状态/HTTP 状态、错误、延迟、token 数、时间与地域。它使用 `deny_unknown_fields`，明确拒绝旧 `userEmail`、`apiKeyPrefix`、`usageAmountUsd`、`settledAt` 以及尚未定义的 `tenantId`/`consumerRef`；`settled` 也不是合法 active 状态。Router 的 metrics 只投影 `gateway_id`，不写 USD cost 或 legacy `market_email`；兼容观测视图也只保留专属 `gateway_id` 列，Gateway 行的 `user_email` 固定为 `NULL`，因此不会进入用户用量或配额身份聚合。

Gateway inventory wire 不序列化 Share owner email、installation owner email、Provider account email 或 Provider API URL。`shareName` 是由 Share ID 派生的稳定 opaque label，不是 Server 的 owner-derived display name；Owner email 仅可作为 Router 进程内的 owner-scope feedback 分组键，不得成为外部 inventory 字段。这条边界必须在未来启用 tenant/seat grant 时继续保持。

Gateway proxy 的当前签名只绑定 Gateway ID、固定 action、原始 body hash、时间戳和 nonce；method/path/share ID 未进入签名域，后续版本必须在 contract review 中决定是否补强。Router 不为 Gateway 伪造终端用户邮箱，因此非免费 Share 的最终用户授权仍由未来平台/新 contract 解决；当前不能宣称跨 Router Token Market 已可用。

### 2.4 旧 Token Market 路径

以下旧注册、bearer、Market host、通知和代理路径均已退役，Router 对已知及其子路径统一返回 `410 Gone`，不会回落到 UI 或 Client tunnel：

```text
/v1/markets*
/v1/market/*
/v1/admin/markets/*
/_market/proxy/*
```

它们只在 frozen baseline、migration 19/21、显式 `410` 和负向退休测试中出现；migration 21 完成后不存在可查询的旧 archive。Share Market、Client Market、`/v1/market-access/*` 与 `/v1/market-wallet/*` 是 Router 内建能力，名称中的 market 不表示旧 Token Market。

---

## 4. Lease:一次性 SSH 凭证

`POST /v1/tunnels/lease` / `POST /v1/tunnels/lease/renew`

Router 不接受长效 SSH 凭据。每次建链前 Server 申请一次性 lease,响应结构见 `TunnelLease`(`src/models.rs:96-114`):

| 字段 | 说明 |
|---|---|
| `id` / `connectionId` / `rotationId` | lease 与连接标识 |
| `routeId` | 逻辑路由标识,见第 6 节 |
| `generation` / `expectedGeneration` | 世代号,用于抢占判定 |
| `subdomain` | 分配的子域名 |
| `tunnelType` | 隧道类型,见第 6 节 |
| `sshUsername` | **必须是合法 UUID**,否则 SSH 层直接拒绝(`src/ssh.rs:426`) |
| `sshPassword` | 一次性密码,`consume_lease` 原子校验并作废(`src/ssh.rs:241`) |
| `expiresAt` | 过期时间 |
| `share` | share 隧道携带 `ShareDescriptor`,client 隧道为 `null` |

`ShareDescriptor` 面向全新部署时必须满足以下约束:

- Router 当前 reader 支持 **Share Contract v2..v10**。Server 普通未协商 descriptor 保持 **Share Contract v7**；账号周期 v8、一次性市场试用 v9、美元等值配额 v10 须显式能力。v4 引入本节 `modelProbe`（Router 仅对 v4 以上执行探针），v5 引入 Grok 媒体权限，v6 增加 scoped quota metadata，v7 增加只展示的 `unobservedTiers` 及 Codex `bankedReset`；它们不参与调度/限流/容量评分。连接测试/模型健康检查保留 v7 兼容。
- Share Market 最低版本仍为 v5（`MIN_SHARE_MARKET_CONTRACT_VERSION=5`），不因账号周期的协商扩展抬高。reader 上限、普通 descriptor 版本与市场最低版本是三项独立约束。
- v8增加`tokenPeriod=accountReset`及userGrants只读`accountQuotaCycle/accountResetCapability/accountUsageBaseline`、`usageQuota.usageWatermark`。安装heartbeat响应同时返回`shareContractVersion=10,capabilities=["accountResetV1","marketTrialV1","usdEquivalentQuotaV1","cacheCreationTtlV1"]`；Server同时检查显式能力、Router URL/installation绑定和300秒有效期。普通v7必须省略新嵌套字段，v7携带accountReset/新字段拒绝，不能静默转换为sevenDays；市场报价不开放动态周期。
- v9 的 `marketEntitlements` 按 entitlement ID 返回 `trialScopeId/email/phase/active/trialTokenAllowance/trialTokenLimit/trialTokensUsed/usageRevision/usageUncertain/appliedAtMs/paidStartedAtMs`。`phase=trial|paid`，Token cap 缺省表示仅时长试用，不能写普通 `tokenLimit:0`。试用累计数和修订只增不减，与普通日／周／月 quota 分开；使用量变化不提升静态 descriptor generation。完整 descriptor 的 Notify＋周期重放持续携带动态试用水位。v9 以下不得携带非空试用状态。
- v10 的普通用户 `policy.valueQuota={limitUsdMicros:"10000000",tariffPolicyId:"api-equivalent-v1"}` 与 `tokenLimit` 互斥，共用原 `tokenPeriod` 和 anchor。它是模型 API 标价等值使用限额，不是钱包余额或 Provider 账单。缺省仍为不限额；零额度非法。grant 可带 Server-owned `valueMetering={quotaPolicyEpoch,valueMeteredFromMs,effectiveFromMs,pricingRevision}` 和 `pendingValueQuota={policy,effectiveFromMs,retainAccess}`；`valueMetering` 只出现在金额模式或已排期金额策略的授权上，Token 授权不带（见 §13.2）。切换到未完整覆盖的自然窗口时保留原 policy，到边界后持久化启用；新授权等待期间不提供无限制访问。首次累计模式从确认的计量起点开始，之后删加授权、改策略、切换单位均不能重设该起点或清空同窗口消费。v7–v9 不接受这些字段。市场托管授权可携带 `valueQuota`（见 §7.1 美元等值车位），但不得带 `pendingValueQuota`：车位额度按合同计量，Server 在该合同首次下发金额策略时以当时为计量起点立即生效，同一 entitlement 的后续 upsert（试用转正式、欠费暂停后恢复、App 范围升级）沿用该起点，新合同重新开始；金额校准对托管授权仍然禁止。
- `cacheCreationTtlV1` 是独立的请求日志能力，不能从 reader v10 或 `usdEquivalentQuotaV1` 推断。协商后可同时附 `cacheCreation5mTokens`、`cacheCreation1hTokens`，两者非负且和等于原 `cacheCreationTokens`；两者必须同时缺省或同时出现。历史合并值保持 unknown TTL，不能虚构拆分；原四桶 Token 总数不变。
- managed upsert 可附 `marketPhase/trialScopeId/trialTokenAllowance/trialTokenLimit`；revoke 不附这些字段。相同 entitlement 不得改变 scope、重新赋予试用或增加原始预算。Server 在转发前持久化请求所属 entitlement 和 phase，迟到试用用量不能计入正式额度。正式授权回执确认后 Router 才从预留款捕获首期费用；失败或未知回执先撤权，确认清理后才释放预留。`usageUncertain` 表示崩溃等情况下消费不能证明完整，须安全回收，不能当作零用量续试。
- `accountQuotaCycle`包含`streamIncarnation/sourceRef/sourceEpoch/bucketId/cycleId/cycleSequence/cycleRevision/startsAtMs/endsAtMs/basis/observedAtMs/eventId`，UTC毫秒半开区间，end可空。缺完整历史时`usageQuota=null`而非0；Router不按本机时间外推窗口，不把本地历史加回Server权威账号quota，不用跨周期max。
- 动态账号投影不改变管理员configRevision/静态descriptor generation/fingerprint。当前复用完整descriptor同步及ACK，Server Notify+30秒重放，**没有独立dynamic envelope或精确usageWatermark ACK**。先检查stream/source/epoch，再按cycleSequence、同cycle revision和usageWatermark合并；合法更高水位允许数值下降，旧周期/旧水位丢弃。source交接要求更新config revision；新stream完整descriptor为restore握手，允许合法静态版本从旧备份恢复，旧stream在Share级永久retire，不能随用户删除/新增复活。
- `schema/0088_account_quota_projection.sql`追加nullable历史`started_at_ms/quota_source_ref/quota_source_epoch/quota_bucket_id`和active/retired stream表。旧created_at秒级日志保持unknown provenance，不乘1000伪造精确。请求日志wire新增可选`startedAtMs/quotaAttribution`；账号用量仍以Server投影为权威。
- `POST /v1/shares/:share_id/account-reset-preview`要求Share owner并经已签名`/_ctl/preview_account_reset`读取Server当前config/cycle/逐用户用量；不消费券、不改policy。设置/preview的`QUOTA_CYCLE_CONFLICT`保持409及白名单只读details/previewRequired，前端应重新预览；市场managed grant仍只读。真实账号接受性与部署另行验收。
- quota 的 `tiers` 只承载真实上游数值；`unobservedTiers` 每项携带 `name`、`label`、`scope`、`capacityPool`、可选 `modelFamily` / `relativeWeeklyCapacity`、`source` 与 `reason`，不包含 utilization/reset。相同 `capacityPool` 或规范化 tier 身份的真实 `tiers` 项必须覆盖展示提示，不得把未观测状态渲染为 `0%`。
- v7 quota 可选携带 `bankedReset`（仅 Codex OAuth）：`availableCount` 为已知且大于零的可用次数，`detailsAvailable` 标识到期明细是否可信，`expiresAt` 至多三个去重后的未来到期时间。该字段只供 Share 私有额度摘要显示，不属于 `tiers`，不得影响调度、限流、容量评分或公开 Share Market DTO；不得传 credit ID、workspace ID 或消费操作。Router 的 quota 解析拒绝未知字段，部署时须先升级 Router，再升级写出该字段的 Server。
- 独立展示扩展 `buddyQuotaPresentationV1`：heartbeat 显式声明且 reader≥7 时，Server 才为 `codebuddy_oauth` / `workbuddy_oauth` 在 quota 写出 `providerUsage`、在 `tiers[]` 写出可选 RFC3339 `expiresAt`。不提高普通 descriptor、reader 上限或市场最低版本；能力绑定 Router URL/installation，300 秒失效。所有 descriptor/claim/lease/runtime 出口对旧 peer 省略新字段，能力变化强制补发已 ACK 的 Share。资源包保留官方 `label`，`resetsAt` 是周期结束，`expiresAt` 是权益失效，两者不能互相替代。
- Buddy `providerUsage` 是账号级、只读的官方逐请求 credit 汇总：必填 `status=complete|unavailable`、`source=buddy_official_request_usage`、`scope=account`、`timezone=UTC|Asia/Shanghai`、UTC 毫秒 `collectedAt`。complete 必须有 `rangeStart/rangeEnd`（ISO 自然日，含首尾至多 31 日）、非负有限的 `usageToday/usage7Days/usageThisMonth/usageTotal` 和非负整数 `requestCount`；unavailable 省略这些值，不能填零。请求数对应整个采集范围，不是今日次数。严格拒绝未知字段，禁止请求/模型明细、prompt、账号标识及原始错误；该摘要不进入公开 Share Market DTO。
- 资源包 `queriedAt` 和官方明细 `collectedAt` 分别记录各自采集时间；用量刷新只更新动态快照并强制完整 descriptor 同步，失败沿用既有错误重试，不提升 configRevision/静态 generation/fingerprint。官方明细沿用 30 分钟成功缓存；手动强制刷新绕过缓存。两套账本包含该账号其他 Share/客户端的活动，资源包周期与自然日不同，金额不要求相等。一类失败不能隐藏另一类的可用证据；新增字段不参与调度、限流、容量评分或本地 Token/USD 计量。默认账号卡片显示各包已用/总额及 `20d12h` 形式的倒计时，随后以 ` · ` 接今日/本月官方 credits，不固定换行。倒计时优先取 `resetsAt`，缺失时才取 `expiresAt`；无有效未来时间则省略，不推导周期。有金额时省略重复百分比，不展开完整期限、采集时间、7 日用量、请求范围和重复说明，完整线格式及校验保持。
- Buddy 私有账号卡片的身份行使用既有 `accountLabel`，与 Server 的 `displayName` 共用安全显示规则：邮箱、脱敏手机号、昵称／姓名、内部账号 ID 依次回退。该值只来自匹配产品／账号／身份代次的托管账号；标签缺失时显示 `-`，不猜供应商名或分享者邮箱。账号信息不进入 `providerUsage` 汇总或公开 Share Market 身份；Buddy 公开能力的 `accountHint` 固定省略，不能回退到邮箱或昵称，不新增线格式字段。
- `capacityPoolId` 是非空匿名标识。同一 Router 下复用相同物理账号或 API key 的不同 Share URL 使用同一值,用于容量与故障域去重；该值在凭据源不变期间稳定，账号绑定或 API key 改变时必须重新派生并同步。
- `bindings` 必须包含 1 到 3 个不同 app 的 `{ app: providerId }` 绑定,app 仅允许 `claude`、`codex`、`gemini`;顶层 `appType` / `providerId` 必须对应其中一个绑定。
- `support` 表示当前对外开启的 App API。关闭某个 API 不会删除对应 binding；至少保留一个已绑定 app 为开启。未开启的 app 不接受直连、Share Market 或 Gateway 请求。
- `appRuntimes`、`appProviders` 与 `appAvailability` 只可声明已绑定 app。访问策略由 Share 级 `freeAccess` 与 `userGrants` 统一定义；v4 延续 v2 的边界,不包含分 app ACL、`appSettings` 或普通 Share 价格字段。
- 独立扩展 `providerIdentityV1`：heartbeat 显式声明后，Server 才在 `appProviders.*[]` 增加可选 `providerIdentity`（不改变 Share contractVersion）。字段为 `schemaVersion=1/familyId/category/displayName/icon?/iconColor?/catalogVersion/revision`；family 来自 Server Registry，category 来自同源展示契约，绝不由 App、协议、模型、名称、URL 或 Logo 推断。所有 descriptor/claim/lease/runtime 出口均按能力投影；未知能力或旧 Router 不收到新字段，能力变化自动通过现有 generation/fingerprint/ACK 补发已有分享。
- Router 将已绑定 Provider 聚合为 Share 级公开 `providerIdentity`，只保留 `schemaVersion/familyId?/category/displayName/icon?/iconColor?/catalogVersion`，不发布 Bundle/provider/account/credential 标识。同 Bundle 多 App 须有相同 Bundle ID 和一致身份；历史多 Bundle/无法证明同源为 `legacy_mixed`，缺失/冲突为 `unknown`，不猜品牌、不自动改绑或停用。分类筛选只读这个单值 category；旧 `providerFamily/providerFamilies` 尤其不可变租约快照语义保持不变。
- 展示身份属于静态 descriptor ACK 内容，但不改变 configRevision、执行/健康指纹、凭据、隧道或健康历史。runtime 是观测而非身份 writer，按 app/providerId/Bundle ID 保留已确认身份，丢弃迟到的静态覆盖；旧 writer 省略扩展不抹掉已知身份。图标只按 icon ID 从本地冻结 catalog 取值，保存色优先于 metadata 默认色；未知 ID 回退首字母，禁止远程 URL/任意 SVG 注入。两产品独立构建，catalog 仅显式联合发布导出，资产哈希由各自本地审计验证。
- v5 的 `grokMediaPolicy` 包含 `imageGenerationEnabled`、`imageEditEnabled`、`videoGenerationEnabled` 三个独立布尔权限，缺失时全部为 `false`。它只授权 Share 使用能力，不能绕过 Server Grok Provider 的对应开关或绑定账号 capability evidence。
- `userGrants[].usage` 是当前周期的 Server 派生快照；可选的 `usageRebase` 是 Server-owned 的官方额度重置基线。Router 必须原样 round-trip 该字段并使用 descriptor 中的有效 usage 做限制判断，不能通过普通设置补丁伪造或修改基线。
- `upstreamProvider`、`appRuntimes` 和 `appProviders` 中的 Provider 投影携带有效 `modelPolicy`，并用 `modelPolicyScope=global|per_app` 与 `modelPolicySource=bundle_global|app_independent|profile_fixed` 明确控制来源。`global` 只统一 Bundle 中可配置的 Surface；Profile 固定策略可不同且必须标记为 `profile_fixed`。这些字段属于静态 descriptor 指纹，单独切换 scope 也必须提升投影并同步 Router。
- v4 的每个可测试 Provider 运行时携带无凭据的 `modelProbe`。它是 Server 供应商测试请求的唯一公开描述,连接示例和 Router 定时测试不得自行维护模型名或请求模板。字段语义如下:

| `modelProbe` 字段 | 语义 |
|---|---|
| `apiType` | 公网 API 协议名:`openai`、`anthropic` 或 `gemini`;分别对应内部 `codex`、`claude`、`gemini` |
| `requestedModel` | Server 供应商配置选定的测试模型,可包含 `@low` 等请求修饰符 |
| `wireModel` | 实际写入公网请求的模型名;请求修饰符已拆入 body 的对应字段 |
| `method` / `path` / `body` | 可直接通过 Share URL 发出的结构化模型测试请求;当前 method 固定为 `POST`,body 上限 64 KiB |
| `stream` / `responseMode` | 是否流式及完成判定:`json`、`anthropic_sse`、`responses_sse` 或 `gemini_sse` |
| `payloadRevision` | 请求模板版本,当前为 `2`;未知版本必须 fail closed |
| `healthFingerprint` | Provider revision、凭据世代和运行时策略的非敏感指纹,用于拒绝配置变更后的陈旧结果 |

  Codex 使用 OpenAI Responses `/v1/responses`,Claude 使用 Anthropic Messages `/v1/messages`,Gemini 使用 URL 编码模型段的 native `generateContent`/`streamGenerateContent` 路径。Router 入库和执行前均校验固定路径、body 中的 wire model、stream/responseMode 一致性及 SHA-256 health fingerprint,并递归拒绝 Authorization、API key、OAuth token、Cookie 或其他凭据字段。
- 调用 app 只由 URL 协议路径判定,客户端提供的 app header 不参与授权。未绑定 app 的直连、Share Market 和 Gateway 请求均被拒绝。

已连接的 Server 使用签名续期 API 在**原 SSH 连接上**续期,不按 lease TTL 周期重建连接。

---

## 5. 建链与激活

```
POST /v1/tunnels/lease          申请一次性凭证
  → SSH 密码认证(用户名 = UUID)
  → tcpip_forward 请求远端端口
POST /v1/tunnels/activate       候选提升为活跃
  → 轮询 POST /v1/tunnels/state 直至 state == "active"
```

SSH 侧约束(`src/ssh.rs`):

- **仅** `auth_password` 认证,用户名必须是合法 UUID
- `channel_open_session` 恒返回 `Ok(false)`(`src/ssh.rs:255-261`)—— 无 shell、exec、sftp、pty
- 只放行 `tcpip_forward` / `cancel_tcpip_forward`
- lease 的 `generation` 必须**严格大于**当前活跃世代,否则候选被判定为陈旧而拒绝(`src/proxy.rs:695`)

Router 在收到 `tcpip_forward` 后绑定本地 TCP 监听(`0.0.0.0`/`::` 归一化为 `127.0.0.1`),注册为候选路由。Client-web 与 Share 隧道需经 `activate` 显式提升；旧 `market-http` lease 会被拒绝，不再创建候选路由。

SSH 断连或 `cancel_tcpip_forward` 时,`ForwardHandle::shutdown` 移除该世代并中止监听任务。

---

## 6. 隧道类型

| 维度 | Client 隧道 | Share 隧道 |
|---|---|---|
| `tunnelType` | `client-web-http` | `http` |
| `routeId` | `client-web` | `share:{share_id}` |
| 子域名 | `{client_subdomain}` | `{share_slug}--{client_subdomain}` |
| lease 携带 `share` | 否 | 是(`ShareDescriptor`) |
| 用途 | Server 属主自己的管理端点 | 对外提供的额度共享端点 |
| 数量 | 每 installation 一条 | 每个启用的 share 一条 |

当前只有 Client-web 与 Share 两种 tunnel type。旧 `market-http` 属于已退役 Token Market 集成，Router 在 lease/SSH 层 fail closed；Client 不应申请或识别该值。

子域名格式与保留字规则见 `src/namespace.rs`:分隔符固定为 `--`,slug 为 6–30 位小写字母数字连字符、必须以字母开头、不得含连续 `--`,组合后须满足 DNS 63 字节限制。保留标签:`admin`、`api`、`cdn-cgi`、`router`、`www`。

---

## 7. 控制平面:Router → Server

Router 复用已建立的反向隧道**同步调用** Server 本地 API,避免等待客户端轮询(`src/ctl_client.rs`)。

已实现方法:

- `POST /_ctl/apply_share_settings` —— 下发 share 配置补丁
- `POST /_ctl/refresh_share_usage` —— 触发用量刷新
- `POST /_ctl/verify_share_account_recovery` —— 对 Share 当前绑定 Provider 执行一次真实恢复探针；仅在账号身份与冷却快照均未变化且探针成功时清除 `account_rate_limit`

**HMAC 规范串**(`src/ctl_client.rs:110-154`):

```
{METHOD}\n{PATH}\n{BODY}\n{TIMESTAMP_MS}\n{NONCE}
```

以 `control_secret` 为密钥做 HMAC-SHA256,结果取标准 base64。请求头:

| 头 | 内容 |
|---|---|
| `x-ctl-installation-id` | installation ID |
| `x-ctl-timestamp-ms` | 毫秒时间戳 |
| `x-ctl-nonce` | 随机 nonce |
| `x-ctl-signature` | 上述 HMAC |

**降级策略**:`Unreachable` / `Timeout` 时回落到异步 pending-edit 队列 + SSE 通知路径;`Rejected` / `Malformed` 直接向 dashboard 返回硬错误。

Router **从不改写** Server 返回的 descriptor,只做校验(`store::apply_share_edit_with_receipt`);若客户端只部分应用了补丁,Router 拒绝落库而非静默持久化。

普通 Share 配置保存使用同一个持久化操作身份贯穿同步 RPC 和异步队列：

- 浏览器冻结编辑时的 `baseConfigRevision`；Router 将其写入 `patch.editContext.expectedConfigRevision`，并用 edit ID 设置 `operationId`。浏览器不得指定 `editContext`。
- `userGrantEdits[email] = { expectedRevision, policy }` 只提交变化的用户；`policy: null` 撤销普通授权，不能撤销 owner 或修改 active 市场托管授权。不得与整表替换的 `userGrants` 混用。未修改用户的 revision、用量及校准记录保持不变。
- `userUsageEdits` 仅用于显式用量校准，和普通配额编辑分离。批量参数默认不勾选，清除基线也必须显式操作。Server 在同一 quota mutation 中校验全部用户及最新用量，任一失败则整笔不落地。
- Server 将成功操作的 `settingsReceipt = { operationId, patchFingerprint, configRevision }` 与 Share 一同持久化；同 ID、同载荷的重试只返回已有结果，不再执行。指纹为递归排序对象键后的紧凑 JSON `[shareId, patch]` 的 SHA-256 小写 hex。回执缓存最多 4096 条；缓存淘汰后，旧操作的冻结配置版本仍会阻止重放。
- RPC 响应及 applied ACK 必须附带匹配回执，不能只凭当前动态用量判断校准是否执行。回执版本的 descriptor 必须满足补丁；若期间已出现更高版本，回执证明历史操作成功，而 descriptor 表达最新状态。晚到的已 applied 响应幂等返回，不覆盖新配置。
- owner 可通过 `GET /v1/shares/:share_id/settings-edits/:edit_id` 查询确切任务；独立 Share 页面对应 `GET /share-api/share/settings-edits/:edit_id`。前端仅跟踪本次 edit，不用路由健康或任意 dashboard 刷新冒充成功，不重开已关闭的编辑器。

部署顺序：先升级 Server，再升级 Router。旧 Server 不识别新补丁时明确失败，不降级成无版本校验的整表覆盖。已有不带 editContext 的历史队列仍按原契约处理。

### 7.1 Share Market managed grants

内建 Share Market(`src/share_market.rs`,`/v1/share-market/*`)在租用/回收时不直接改 Server 本地文件,而是经 pending Share edit 下发 **managed grant** 操作:

| 字段 | 含义 |
|---|---|
| `patch.managedGrant.action` | `upsert` 授予拼车位 / `revoke` 回收 |
| `patch.managedGrant.entitlementId` | 稳定 entitlement ID,与订阅绑定 |
| `patch.managedGrant.policy` | 拼车位并行/Token 限制、周期及显式 `allowedApps`(upsert 必填) |
| `patch.managedGrant.durationSeconds` | 有固定服务期限时的相对时长；Server 在应用授权时按 `appliedAtMs` 写入绝对 `expiresAt`，policy 本身不得携带 `expiresAt`；revoke 不得携带 |
| `patch.managedGrant.termination` | 协商 `shareTerminationV1` 后，仅 revoke 可带的原租约结算 fence，详见下文 |
| Server 落库 `userGrants[].manager` | 固定为 `routerShareMarket` |

Server 要求:

1. 普通 `share/settings` 入口拒绝带 `managedGrant` 的补丁。
2. pending-edit 应用路径接受 managed grant,写入/移除 `routerShareMarket` grant。
3. 普通用户编辑不得修改或删除 **仍为 active** 且 `manager=routerShareMarket` 的 grant。撤销后的 tombstone（`active=false`）保留作历史/用量 deprecated 标记，不再占用该邮箱；普通编辑可将同一邮箱写成新的 Manual `shareto`。不得把 tombstone 重新点亮为市场授权；再次出租仍只走 `managedGrant.upsert`。
4. edit-ack(`POST /v1/shares/edit-ack`)成功后,Router 将订阅从 `grant_pending` 推进到 `active_free` 或 `active_postpaid`,或完成 revoke 后释放座位。

#### Share 立即退租的最终消费回执

`shareTerminationV1` 是独立能力，不可从 reader v10 或 `marketTrialV1` 推断。heartbeat 的 capability 缓存仍须绑定 Router URL、installation，最多有效 300 秒。协商后，SQLite **committed** Server 才能在权威读取和常规 Share 同步中附 `terminationArchiveSinceMs`（UTC 毫秒）。归档起点是持久化能力的真实起点，不能回填到付费周期之前；旧 Router 必须省略该字段。该能力变化进入静态描述符指纹。发布顺序为 Router → Server，与普通设置补丁的升级顺序独立。

`managedGrant.termination={terminationId,cutoffAtMs,periodStartMs,periodEndMs,policy}`，policy 是原租约冻结的执行额度，而非管理员当前展示价目；只允许 revoke，满足 `0 <= periodStartMs <= cutoffAtMs <= periodEndMs`。Server 先持久化原 entitlement 撤权，并保留 pending ACK 标记，直到原操作的成功 ACK 被 Router 接收。

applied ACK 必须带 `terminationReceipt={terminationId,entitlementId,shareId,email,operationId,cutoffAtMs,periodStartMs,periodEndMs,revokedAtMs,archiveSinceMs,quotaUsedBeforePeriod,windows}`。`windows=[{startsAtMs,endsAtMs,used}]` 是不重叠且排序的执行额度窗口；累计额度使用付费周期边界。`used`、`quotaUsedBeforePeriod` 均为规范十进制字符串：Token 模式为数量，美元模式为已结算 USD picos，**都不是钱包金额**。前者只汇总本周期截止前正式付费的请求，后者是更早付费请求的累计消耗，用于累计额度的期初剩余额度。

独立归档保留每次 admission 的 request、原 entitlement、邮箱、阶段、开始时间、修订及最终状态，不依赖有限展示日志或 Share 内 35 天保留窗口。必须排空所有原 entitlement 的请求，包括试用和截止附近竞态；USD 读取执行 ledger 的全部已结算尝试，缺价格／未结算／不确定不能当作零。只有具备最终失败且零消费凭据的未执行请求才能证明零值。

等待期间返回 rejected ACK，`errorCode=market_termination_pending,retryable=true`。这表示操作可能已在 Server 撤权，**不能**删除 Server 的原操作回放记录，或使 Router 改写同一 operation 的 expectedRevision。Router 保持原 pending edit 和 dispatched operation 重试；成功回执按原 installation、edit revision、operation、租约、计费区间验证并不可变落库。普通描述符缺 grant 或 Share 删除不能替代回执。Server 在活跃市场授权或该成功 ACK 未确认期间拒绝删除 Share，错误码 `cc_switch_share_market_settlement_pending`。

Server 发出最终 applied ACK 前须将其写入绑定原 Router URL／installation 的持久化 outbox。重启后即使 Router 已不再返回该 pending edit，仍重放原 ACK；Router 幂等确认后，先持久化清理 Server 的 pending 标记，最后删除 outbox。不能仅靠内存中的 HTTP 成功结果解除或永久遗留删除门禁。

最终回执确认后，安全回收和财务结算是独立提交点：财务异常保留已确认撤权及原负债，不阻止车位复用，不允许责任裁决绕过资金对账。钱包资金、额度折算、补偿比例、争议及成熟期仍只由 Router 管理，完整规则见 [Share 退租](docs/share-market-immediate-termination.md)。

一个拼车位授权整个 Share,不出售单一 App。报价会冻结当时全部已启用 App 的供应商与模型条款(`service.apps[]`),租客不能再通过 `requiredApp` 收窄授权；该旧请求字段仅作为兼容输入保留并被忽略。Router-managed grant 对三个核心 App 显式下发 `allowedApps=[claude,codex,gemini]`,Server 再以目标 App 当前是否已绑定且启用作为实际可调用边界，因此 Share 后续启用的核心 App 无需重建租约即可使用。升级前的单 App 活跃 grant 通过幂等 upsert 原地扩权,不 revoke、不暂停服务或账单；迁移时已经在途的 v1 初次授权或恢复授权可完成一次旧范围确认，但不会标记为扩权完成，下一轮立即原地扩权。v2 租约不接受该兼容行为。

用户“已消耗 Token / 周期起点”重基线可由 Server Provider 或 Router owner 编辑入口显式提交 `userUsageEdits`；Router Share Market grant 仍只读。重基线变化会带来新的 descriptor generation，普通请求 usage 计数不会触发同步风暴。

浏览器侧 HTTP 契约(用户 Session):

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/v1/share-market/listings` | 公开 catalog(含商品准入与可用性计算后的 `canRent`) |
| `POST` | `/v1/share-market/listings` | 添加 Share 挂牌(1–20 座位) |
| `DELETE` | `/v1/share-market/listings/:id` | 停止挂售；空闲车位报价 revision 递增，未提交 quote 立即失效 |
| `POST` | `/v1/share-market/listings/:id/reopen` | 原子恢复原 listing，可修改并重新发布旧车位，同时添加新车位 |
| `GET` | `/v1/share-market/owned-shares` | 「添加 Share」候选与挂售状态(`canCreateListing`、`activeListingId`、`reopenListingId`、`hasActiveRentals`、`expiresAt`、`appCapabilities`) |
| `POST` | `/v1/share-market/listings/:id/seats` | 为 active listing 添加拼车位；兼容旧调用方在 closed listing 上添加并恢复 |
| `PATCH`/`DELETE` | `/v1/share-market/seats/:id` | 编辑/删除空闲座位 |
| `POST` | `/v1/share-market/seats/:id/quote` | 冻结车位及 Share 全部已启用 App 的服务条款 |
| `POST` | `/v1/share-market/seats/:id/rent` | 租用 |
| `POST` | `/v1/share-market/subscriptions/:id/release` | 租客归还 |
| `POST` | `/v1/share-market/subscriptions/:id/force-revoke` | Owner 强制回收,可同时拒绝该买家后续 Share 租用 |
| `GET` | `/v1/share-market/subscriptions/:id/value-usage` | 当前付费账期的 API 等值估算；仅该租约买家或卖家，所有响应 `private, no-store` |

`alreadyListed` 作为兼容字段,在当前 owner 有 active listing 或 Share 仍有非终态订阅时为 true；新 UI 以 `canCreateListing`、`activeListingId` 与 `reopenListingId` 决定动作。只要存在未删除的 closed listing,`POST /listings` 返回 `share_market_reopen_required` 及原 `listingId`,不得创建重复 listing。

恢复请求至少包含一个 `existingSeats[]` 或 `newSeats[]`。旧车位项提交 `seatId`、当前 `offerRevision` 和完整 `seat` 条款；只有 `disabled`、未退休、无当前订阅且属于该 listing 的车位可重新发布。Router 使用 Immediate 事务校验 owner、Share 状态/归属、公开免费策略、待应用编辑、Client contract 版本、Token 周期、Share 并发上限、付费资料、20 车位上限及其他 active listing,再统一更新旧车位、插入新车位并激活原 listing。任何失败都会完整回滚。进行中的租约既不阻止恢复,也不会被恢复操作修改。

停止挂售是幂等且可逆的。首次停止将 `available` 车位改为 `disabled` 并递增 `offerRevision`,同时把该 listing 下所有 active rent quote 改为 `expired`；重复停止不会再次递增 revision 或重复写审计事件。恢复旧车位时再次递增 revision,因此停止前生成的 quote 即使仍在客户端内存中也不能在恢复后提交。

**美元等值车位**：车位条款可用 `valueQuota={limitUsdMicros,tariffPolicyId:"api-equivalent-v1"}` 代替 `tokenLimit`（二者互斥，共用 `tokenPeriod`，同样不接受 `accountReset`），表示按请求时模型 API 标价折算的用量上限，不是租金或钱包余额。发布、加车位、编辑、恢复与报价含美元车位时，Router 先经 `/_ctl/preview_value_quota` 确认该 Share 的金额能力（存储、价目、文本运行时）全部就绪，且 Server 在 `checks[]` 中声明 `{"code":"market_seat_quota","ready":true}`；否则返回 409 `share_market_value_quota_unavailable`（`details.reason` 为未通过的检查或 `server_upgrade_required` 等）。额度与条款一起冻结到报价和 Subscription（`value_limit_usd_micros`），报价时的已发布价目修订只记录审计（`valuePricingRevision`），执行始终使用 Server 当前已确认价目。旧式计时试用先执行冻结的试用 Token cap，结束后切换为车位额度；新的一次性试用按下文跟随车位单位，普通周期额度同时生效。金额模式下买家并发为 `min(parallelLimit,4)`，单个已准入请求可能小幅超出（软上限）。

**试用跟随单位**：`GET /v1/market-trials/policy` 与 Owner 的 `GET/PUT /v1/admin/market/trial-policy` 增加 `shareValueUsdMicros`（规范非负十进制 micros，默认 `"1000000"` 即 US$1；上限 `"1000000000000000"`）。旧管理客户端 PUT 省略此字段时保留当前值，不覆盖回默认。美元车位的 `trialValueLimitUsdMicros` 覆盖该默认，省略／null 继承，`"0"` 仅按时长；不得同时携带 `trialTokenLimit`，Token／无限量车位不得携带美元试用覆盖。0 小时关闭试用；所有规则变化仅作用于新报价，旧快照与合同不重写。

- `trial` 冻结条款保留 v1，增加可选 `valueAllowanceUsdMicros`、`valueBudgetUsdMicros`、`valueInherited`、`unitLocked`。美元条款成对出现，Token 额度为 0；旧 Token 快照继续按原语义解析。首次有试用时长的合同锁定「买家 + Provider + Share」的预算单位；改租另一单位只给剩余时长，报价标记 `unitLocked=true`，不得领第二份预算。旧请求迟到结算耗尽原单位预算时，Router 对账同样结束已切换单位的时间试用。
- 有正金额预算的美元试用额外要求 Server preview 声明 `{"code":"market_trial_value","ready":true}`；缺失返回同一 409、`details.reason=server_trial_upgrade_required`，不静默降为时间试用。`market-value-quota-readiness` 的 `valueTrialReady` 与基础 `ready` 分开。纯时间、免费和旧冻结 Token 试用不要求该新增声明。
- managed grant／`marketEntitlements` 增加可选 `trialValueAllowanceUsdMicros`、`trialValueLimitUsdMicros`，正金额成对且 `limit <= allowance`，与 Token 试用字段互斥；entitlement 另带单调 `trialValueUsedPicos`（默认 0，零时省略），和 Token 共用 `usageRevision`。转 paid 操作不再下发预算，但 Server 保留原冻结字段，迟到 trial 请求仍计入原预算；不能改单位、用户、scope，不能从 paid 回 trial。
- Server 在同一 SQLite IMMEDIATE 事务中检查周期额度、当前试用合同和原 scope 的累计预算，扣除各自已结算与在途预占，按最小剩余量预占；重试不重复预占。Market 周期账本 scope 带 `entitlementId`，新合同不混入旧合同（包括同毫秒及迟到结算）；普通授权 scope 保持不变。试用累计索引跨合同保留，不依赖可裁剪请求日志。结算水位持久镜像，启动／清扫器补偿恢复，较低修订不能重新开放预算。
- 时长或预算耗尽沿用 trial → activating → paid：先冻结首期资金、确认付费授权生效后才扣款；资金不足安全撤权。Subscription 的 `trial` 返回可选 `valueAllowanceUsdMicros`、`valueUsedUsdMicros`、`valueRemainingUsdMicros`；已用向上、剩余向下取整至 micros，不把金额用量误显示为钱包扣费。

**折扣与价值估算**：标称折扣 = 30 天租金 / 30 天满额 API 价值；day ×30、week／sevenDays ×30/7、calendarMonth／thirtyDays ×1，lifetime、免费或 Token 车位不显示标称折扣。旧日租按日价 ×30。BigInt 计算，正租金不显示 100% 免费；比例不低于 100% 时只中性说明，不显示折扣徽标。它不是实际使用折扣或收益承诺。

`value-usage` 仅估算当前钱包日历月账期，从 `max(periodStart,paidStartedAt)` 到 `min(now,periodEnd,endedAt?,releasedAt?)`（结束／释放时间存在时截断，避免计入后续租用），以 Router 保留日志和同源模型目录计价（按窗口末端有效价，不是每个请求的 Server 冻结执行价）。返回 `subscriptionId`、`periodStart/End`、`rentMicros`、`valueUsedMicros`、`ratioBps`、`pricedCoveragePercent`、`pricingRevision`、`pricedAt`、`partial` 及可选 `reason`（`trial`／`no_paid_period`／`no_usage`／`unpriced_usage`）。有用量但均未定价时不伪造 0；超保留期／部分未定价标记 partial。当前 grant 被停用或移除不抹掉冻结区间里的保留记录。买卖双方界面始终标注“估算”，不参与 Server 权威额度、扣款、退款或 Provider 结算。

免费和付费拼车位都可通过 `serviceDurationDays` 设置 `1..=365` 天固定服务期限；省略或传 `null` 表示无固定期限。`tokenLimit` 与 `valueQuota` 均省略时 Router 忽略提交的 `tokenPeriod` 并归一为 `lifetime`；只有设置 Token 限额或美元等值额度时才校验该 Server 支持的周期。报价参数在租用时冻结到 Subscription；固定期限从 Server 首次应用该 managed grant 时开始计算：Router 只下发相对的 `durationSeconds`，Server 以 `appliedAtMs + durationSeconds` 写入绝对 `expiresAt`，Router 在 edit-ack 中校验一致后冻结到 Subscription。授权下发前的等待不消耗期限，账单暂停不顺延。Router 在到期前 24 小时写入一次 `service_term_expiring`，到期后终止付费合约并根据控制操作是否可能已送达选择直接释放或安全 revoke；付费尾段会精确结算到 `expiresAt`，账务与 Share worker 的先后顺序既不会漏计也不会越界计费。`grant_pending`、活跃、账单暂停、恢复中及控制失败状态均不得在到期后重新获得访问，回收失败时也不会把座位提前恢复为可租。

### Client Market 钱包购买与展示契约

- `POST /v1/client-market/quotes` 的每项返回 `trialSecondsRemaining`（实际剩余健康服务秒数）；`walletFunding` 的金额字段使用整数单位字符串，`fundingRail=platform_wallet` 才可进入新的付费购买界面。免费报价可无钱包字段。报价只预留资源，不冻结用户资金。
- `POST /v1/client-market/quotes/:id/commit` 保留报价修订校验与幂等键；所有付费订单（含试用）须有完整首期可用余额，试用不预留资金，非试用在提交事务中预留当期金额。资金竞争返回 `409 / MARKET_WALLET_REQUIRED` 及资金摘要，调用方保留草稿、重新报价并由用户确认。网络错误或结果未知时重试同一请求及幂等键，不能换单重建。
- Client 租约读取接口向租客返回可选 `walletContract`：`id`、`status`、`trialSecondsRemaining`、`renewalPolicy`、`periodEnd`；不向 Host Provider 暴露买家私有合同投影。试用没有确定的墙钟结束时间。独立的 `walletContractId` 仍仅用于已开始付费的争议入口，不用于判断试用合同是否存在。
- `GET/PUT /v1/market-wallet/contracts/:id/renewal` 为买家专属月租管理接口，读取包含剩余试用；更新携带 `expectedPolicy` 防止覆盖其他会话的修改。
- 商品页不展示 Provider 收款方式；平台充值渠道只在钱包充值流程出现。充值返回时重新报价，不自动购买；密码不得通过 URL 或持久化草稿传递。

### 7.2 Share / Client Market 统一准入

准入按 share/free、share/paid、client_host/free、client_host/paid 隔离。免费默认 blacklist，付费默认 whitelist；关系支持邮箱预授权，首次使用绑定已验证用户。每个作用域可设置 inherit / allow / deny。准入不创建信用或债务，余额是否足够由钱包报价与提交事务判断。

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | /v1/market-access/dashboard | 策略、关系与规则 |
| GET | /v1/market-access/inbox-summary | 待处理申请数量 |
| PUT | /v1/market-access/policies/:product_kind/:pricing_kind | 更新准入模式 |
| POST | /v1/market-access/counterparties | 添加邮箱关系 |
| PUT | /v1/market-access/counterparties/batch | 原子更新多名买家及其规则 |
| PUT | /v1/market-access/counterparties/:id | 更新关系或撤销 |
| POST | /v1/market-access/requests | 创建准入申请 |
| POST | /v1/market-access/requests/:id/approve | 批准商品准入 |
| POST | /v1/market-access/requests/:id/reject | 拒绝申请并记录原因 |
| POST | /v1/market-access/requests/:id/cancel | 买家取消申请 |

写入要求 expectedRevision，批量冲突整体回滚。审批不接受旧 creditLine 参数。Session 与 scoped API Token 继续按 read/write 隔离。GET/PUT /v1/account/provider-contacts 只管理业务联系方式，不含收款账户、二维码或密钥。

### 7.2.1 Market Provider identity 与排名 API

这里的 Market Provider 是 Router 市场供应商主体，不是 Share descriptor 中的 upstream Provider 配置。公开身份使用稳定 opaque `mp_*` ID；Host/Share/资金 alias 只在 Router 内部归并。身份冲突不得按邮箱或显示名猜测，公开 wire 不得返回 canonical email、Router user ID、supplier user ID、余额、信用或合同金额。

| 方法 | 路径 | 契约 |
|---|---|---|
| `GET` | `/v1/market-providers` | 公开当前 last-good generation 和 Provider 排名列表；strong ETag，允许 public cache |
| `GET` | `/v1/market-providers/:id` | 公开 Provider 详情、Share listing 与 Client Host 国家实时供给 |
| `GET` | `/v1/account/market-provider` | 登录用户自己的 Provider 资料、当前排名条目与门槛 |
| `PUT` | `/v1/account/market-provider/display-name` | 修改公开名称（`{displayName}`，空字符串恢复默认） |

v1 权重固定为服务质量 45%、有效选择 30%、履约 15%、供给广度 10%；正式名次要求至少 3 个不同 Router 买家账户和 7 个观测日。免费选择权重为付费选择的 0.25；pending Share grant、自租和一天内释放不算有效选择，`grant_failed` 进入履约失败分母。自租按用户 ID 和双方 canonical email 双重排除；Share 历史质量、选择、履约和性能按显式的 listing Provider 半开时间窗归属，关闭、重开或换主不会移动旧证据；当前供给只统计与公开 catalog 一致的 active Share/可见 seat。这里的账户去重不是 KYC 或完整反女巫证明。TTFT/TPS 明确为 display-only。官方身份没有 score boost，探索展示不改写 `rankPosition`。

一个 generation 只有在完整 entries 与 `published` 状态同事务提交后才可见。`building` 或 `failed` generation 不得替换 last-good；超过 30 分钟只标记 `stale`，不能用不完整新数据覆盖。没有任何已发布代次时端点返回 503。推荐模式由 `CC_SWITCH_ROUTER_MARKET_PROVIDER_RECOMMENDATION_MODE=off|shadow|on` 控制，默认 `shadow`；Share/Client 响应携带 `recommendationMode`，只有 `on` 改变推荐顺序，显式价格/空闲/在线率排序不受影响。

所有供应商共用一份平台余额，不提供分户资金汇总。完整算法和运维边界见 [docs/provider-ranking.md](docs/provider-ranking.md)。

### 7.3 平台钱包与商品生命周期

Share 与 Client Host 付费报价冻结商品条款及资金需求。提交只接受当前钱包报价，余额竞争或报价过期要求刷新，不回退到商家直付。报价不冻结资金；提交在 Immediate 事务内创建唯一合同并冻结开通款。远端服务确认后才扣款，不确定失败保留 hold，清理确认后幂等结算与释放。

月租取消仅关闭续费并保留当期服务，试用、按秒和暂停服务走立即清理。续费通过 GET/PUT /v1/market-wallet/contracts/:id/renewal 管理，校验 buyer 与 expectedPolicy。试用 Share 也返回买家私有 walletContract，公开投影清除钱包合同与争议信息。改价只影响新报价，不改写既有合同。

Host 历史从月租扣款与按秒 posting batch 计算 chargesMinor、refundedMinor 与 netChargesMinor，履约分账不重复计入消费。未履约款只回平台余额，不提供现金退款。Provider 成熟收益自动入余额，无提现或手动转入 API。

GET /v1/market/config 返回 currency、usdCnyRateMicros、fxRateSource、fxRateSourceTimestamp、fxRateFetchedAt 和 fxRateFresh，设置 no-store。汇率每 15 分钟更新，记账始终为 USD。

旧商家支付模块、路由、环境变量和页面已经删除。历史数据库记录仅保留审计证据，不自动迁为钱包资金。详见 [清理契约](docs/market-wallet-retirement.md)。

### 7.4 Market 系统事件与 Client 公开聊天室

Share Market、Client Market 和统一账务不创建商品级聊天室。Router 只按 `installation.id` 建立唯一 Client 公开房间；Share Market 表格和 Client Market Provider/租客入口均解析到对应 `GET /v1/chat/clients/:installation_id/room`。同一 Client 的多个 Share 必须返回同一个 room ID。

关键业务事件与原业务写入处于同一 libSQL 事务,先进入 `client_chat_system_outbox`,再由后台幂等物化为系统消息。Share 服务启停、到期、供应商变更、离线/恢复；挂牌、拼车位、租用、授权、回收；Client 开通、清理开始/成功/失败、强制释放均属于该事件流；资金和私有争议信息使用独立通知，不进入公共聊天室。系统消息自动关注 Owner、Provider、租客和 actor,产生未读,但不触发真人聊天提醒邮件。

事件 payload 采用公开字段白名单：商品、供应商公开邮箱、业务联系方式与脱敏服务状态。不发布买家身份、钱包余额、付款 reference、凭证、私有争议或原始诊断。商家支付方式及收款图片资产 API 已移除。历史消息保留审计记录，读取时继续执行公开投影及凭据过滤。API Key、OAuth/Session token、Cookie、Authorization、密码、secret、私钥和 SSH/lease 凭据不得进入事件 DB、API、日志或 UI。后端入 outbox 前拒绝敏感字段和带凭据 query/userinfo/fragment 的 URL，错误文本命中凭据片段时整体替换为固定占位；Web UI 继续独立执行字段、文本和 URL 二次过滤。

### 7.5 Client 自升级状态与 rollout circuit

Server 使用 installation Ed25519 身份向 `POST /v1/installations/upgrade-task-report` 上报升级状态，签名 action 固定为 `installation_upgrade_task_report_v1`。canonical payload 顺序为 `taskId`、`status`、`restartPending`、`logs`、`targetCommitId`、可选 `failure`、`restartAfter`、`updatedAt`；为了兼容旧 Server，`failure` 不存在时不得序列化该字段，原有签名串保持不变。

`failure` 结构为 `failureCode`、`stage`、可选 `exitCode`、`diagnostic`。它只允许出现在 `status=failed` 的报告中；原因码只接受小写 ASCII、数字与下划线，code/stage 最长 128 字节，diagnostic 必须非空且最多 4096 字节。Router 在 signed report、Client Web reconcile 和 status read 三条路径中持久化并返回同一对象；终态任务不得被迟到的 `running` report 清除诊断。

Router 在向 Client 发起目标 commit 可确定的升级前检查历史失败。同一目标（完整 SHA 与至少 7 位的大小写不敏感前缀视为同一 commit）在至少两个不同 installation 上出现同一种确定性原因时返回 `409 Conflict`（code=`CLIENT_UPGRADE_ROLLOUT_CIRCUIT_OPEN`）并打开 rollout circuit。当前确定性集合只有：

- `startup_contract_preflight_failed`：staged binary 的只读启动契约预检失败。
- `replacement_exited`：替代进程或服务在健康检查完成前已明确退出。

`upgrade_failed`、`resource_preflight_failed`、`health_check_timeout` 等不进入熔断计数。熔断按目标 commit 隔离，不影响其他 release；Router Web 收到该 409 后禁用当前 installation 的连续重试。

Router Web 的升级按钮只把进行中的任务写入 `sessionStorage`（`cc_switch_router_client_upgrade_v3:`）。`failed` / `idle` 不落盘，刷新走 discovery：没有 running task 则回到普通「升级」，不把历史失败闩回「重试升级」。同一标签里的失败闩只在隧道在线且 `installations.status` 上报的 `commitId` 相对失败时快照发生变化时静默清除；不使用 `updateAvailable=false`（已经是最新仍预检失败时会抹掉诊断）。`CLIENT_UPGRADE_ROLLOUT_CIRCUIT_OPEN` 的 `retryBlocked` 保持到离开该标签。`installation_upgrade_tasks` 终态仍以 Server 上报为准，手动换二进制不会把失败任务改成 success。

---

## 8. 探针:`/_share-router/*`

Router 经隧道拉取 share 侧运行时数据。Server 需实现:

| 路径 | 用途 |
|---|---|
| `GET /_share-router/health` | 健康探测(`src/main.rs:858`) |
| `GET /_share-router/request-logs` | 请求日志拉取 |
| `GET /_share-router/share-runtime` | share 运行时状态(额度、用量、模型健康) |
| `POST /_share-router/model-health` | 兼容的单 Share、单 App 实时模型探测 |
| `POST /_share-router/model-health/batch` | 模型健康批协议 v1,仅用于滚动升级回退 |
| `POST /_share-router/model-health/batch-v2` | 当前模型健康批协议 v2;同一 installation 最多 256 个目标 |

校验方式与第 7 节相同(同一 HMAC 契约)。`request-logs` 与单目标 `model-health` 使用 `x-cc-switch-share-id` 标识目标 Share;`share-runtime` 在多 Share Server 上使用 `shareId` query;batch 端点从签名 body 的 `targets[] { shareId, appType }` 取目标,不依赖单 Share header。v2 body 还必须携带 `contractVersion=2`;Router 仅在 v2 返回 404/405 时回退 v1,其他协议错误不得静默降级。

### 8.1 连通性与模型健康是两套信号

Router 每 30 秒对 Share/Client 隧道调用一次 `GET /_share-router/health`。这只验证隧道和 Server HTTP 控制端点的**连通性**,结果进入 route/share health 时间线,不消耗模型请求,也不计入模型健康热力图。

模型健康按 UTC 半小时槽运行,每个完整监测日固定 48 槽。Router 的模型健康后台任务也每 30 秒唤醒,用于在启动、短暂故障或多实例接管后及时领取当前槽；`share_model_health_slots` 的 `(share_id, slot_start)` 主键和 claim token 保证同一 Share 每个半小时最多保留一个有效投影。目标先同步资格 epoch,实际轮到 installation 分块执行时才 claim；claim 会原子校验 epoch 仍覆盖当前槽,槽结束后不再领取排队目标。pending claim 超过 10 分钟后才允许接管,覆盖单批次 7 分钟总预算及持久化余量；`outcome=unobserved` 的控制面失败可在当前半小时内立即重新领取,真实 Provider `success`/`failure` 不可覆盖。每个活跃 Share 只选择一个已启用且携带可执行 `modelProbe` 的 App,优先级固定为:

1. Codex → `appType=openai`
2. Claude → `appType=anthropic`
3. Gemini → `appType=gemini`

Server 与 Router 内置相同固定顺序,不能人工选择。选择仅限 Share 自身已启用、有非空有效绑定且携带当前 API 类型可执行探针的 App；没有探针的高优先级 App 会被跳过,但限流、额度不足和健康失败不切换 App。Router 的调度与资格 epoch 共用同一选择函数。三者均不可测试时关闭当前资格 epoch,不会用硬编码请求补位。Router 先按 installation 分组,最多并行处理 16 个 installation；控制路由优先使用 installation 的 Client 隧道,再回退最多两个当前在线的 Share subdomain。同一 installation 内的 256 条分块保持顺序,每个已 claim 批次的全部 v2 路由尝试和可选 v1 回退共享 7 分钟总预算,且最晚不得超过槽结束后 5 分钟。v1 没有 canonical observation 幂等保证,因此每批最多回退一次；模糊传输失败记为 monitoring gap,不经另一 Share 路由重复消费模型请求。Server 在 batch 内按 Provider runtime 分组,最多并行探测 3 组,完成后按 App/Provider key 确定性排序。单个目标已删除、禁用、绑定失效或不再是自动代表接口时 Server 省略该结果；单个 Provider 组执行异常也只省略该组,不得让其他 Provider/Share 连带失败。Router 只把对应缺失目标记为 monitoring gap。

`cycleId` 是确定性的 `utc-{slot_start}`。Server 按 Provider 运行时去重执行,一次实际探测可以投影到多个 Share；相同 cycle 的 HTTP 重试复用 health fingerprint 完全相同且内容稳定的 Provider 结果。Router 主调度,Server 自己的半小时 Provider scheduler 兜底,最近 45 分钟已有相同 health fingerprint 的 Router cycle 时跳过对应 Provider。本地先执行时,Router batch 在同一 Provider 锁内复用同 UTC 槽、同 fingerprint 的原始 scheduled 结果,保留测试时间、状态和重试次数；手动测试和确认结果不替换已固定的 cycle 结果。本地日志来源仍为本地,Router cycle 投影记录不会把原始来源伪造成 Router 实测。Server 有活跃 Share 的 Bundle 不再额外检查 Share 未使用的代表接口,无活跃 Share 时按同一优先级检查 Bundle 的一个代表 App。旧 `testApp` 仅供兼容,不控制自动调度。既有同接口重试与瞬态失败确认可能额外发起请求；该策略不承诺消除上游全局限流。历史其他 App 的健康记录不删除。

v2 每条结果必须携带 `observationId`、`outcome`、`failureDomain`、`reasonCode`、`evidenceScope=provider_runtime` 与 `evidenceVersion=2`。`observationId` 是 installation、cycle、App、Provider 和 health fingerprint 的稳定 SHA-256；Router 会重新计算,并严格匹配 App、Provider、`requestedModel`、`actualModel`、模型策略和 health fingerprint。`actualModel` 在固定上游策略下是固定上游模型,在透传策略下是 probe 的 wire model。一次 Provider observation 只写入 `share_model_probe_observations` 一次,各 Share 槽只保存其投影。相同 ID 的状态、时间、模型、延迟或故障分类发生漂移时整笔投影回滚。

故障域固定为:上游/网络/限流属于 `upstream`,账户额度阻断属于 `quota`,鉴权、模型名和 Provider 协议配置属于 `provider_config`,隧道与控制请求属于 `control_transport`,Router 编解码/校验/持久化属于 `router_monitor`,无法稳定分类才使用 `unknown`。前三类是已实际观察到的上游侧失败；后两类没有观察到 Provider,必须写成 `outcome=unobserved` 并计入 monitoring gap,不得伪装成上游失败。v1 回退结果标为 `share_legacy`/`evidenceVersion=1`,继续展示但不得用于供应商信誉、SLA、退款或结算。

每次监测配置的有效期由 probe epoch 精确表达,包含 App、Provider、容量池、测试模型、模型策略和 health fingerprint。启用、暂停、恢复或配置切换均开启/关闭 epoch；当前槽已经被领取时,切换从下一槽生效。完整资格日分母为 48；首次启用日、暂停日和恢复日仅计算实际落在 epoch 内且已经开始的槽。Share 运行前、暂停窗口和未来槽不进分母。

Dashboard Share 与 Share Market listing 同时返回 Router 只读投影 `modelHealthStatus`，包含 `state`、可选的 `appType/model/healthFingerprint/checkedAt/staleAt/statusCode`、`reasonCode` 和最新优先的 `recentResults`。两处使用与自动调度相同的代表 App，不用 Share 主 App、信息最丰富的 App 或隧道在线状态代替模型检查。只读取匹配当前配置、当前资格 epoch、installation、容量池和模型的 v2 immutable observation：最近三次实际检查全失败为 `unavailable`（红），存在失败或 degraded 为 `degraded`（黄），已取得的最近检查全成功为 `healthy`（绿）。恢复时保留最近失败的黄色窗口，直到其退出最近三次；控制面 monitoring gap 与 pending 不算实际成功或失败。最新实际检查满 65 分钟、没有当前证据、配置已切换或不支持探针时为 `unknown`（灰），不得按缺失数据默认绿色。`checkedAt/staleAt` 为 Unix 秒；摘要不包含原始上游错误、账号或凭据。

该监测投影与 Server 的调度容错分开：首个瞬态 429 可以暂时保留调度资格，但检查仍为 failed，不可改写为 success。Router 的 runtime 摘要不能覆盖固定槽已有的 canonical 检查，也不能因省略对应模型而删除其状态；历史上被错误 runtime 摘要覆盖的检查展示按 immutable observation 还原读取，不改写原始证据。模型降级或待验证仅为 service readiness 提示，三次确认失败仍为 `provider_unavailable` 阻断提示；不引入新的调度或限流策略，不修改资金结算。

客户端、Share 直达页和 Share Market 另返回只用于卡片展示的 `liveModelHealthStatus`。它以 `modelHealthStatus` 为基线，可采用时间更新且匹配当前 installation、代表 App、Provider、模型策略、请求/执行模型、health fingerprint 和当前 epoch 的手动证据。`source` 为 `periodic`、`server_manual` 或 `router_connection_test`，可携带 `latencyMs`。手动结果有效期同为 65 分钟；配置切换、暂停/恢复后不能复用旧 epoch 的结果。新配置即使要到下一半小时槽才开始周期监测，也可先显示可信的手动验证结果。首次正常验证为绿，响应慢为黄，真实失败先黄；已有近期失败时，手动成功仅显示黄“恢复中”，重复点击不能清除恢复窗口或制造三个周期成功。更新的周期结果优先。周期统计、service readiness、调度及资金仍只使用各自原有证据，不读取该实时展示投影。

`GET /_share-router/share-runtime` 根对象新增可选 `manualModelHealth` 数组与 `modelHealthDegradedThresholdMs`（1000..30000，来自 Server 当前配置）。每条手动证据包含 `observationId/appType/providerId/healthFingerprint/requestedModel/actualModel/status/statusCode/latencyMs/checkedAt`，不含账号、凭据或原始错误。仅从当前配置已接受的真实 `cc-switch-manual` 检查日志生成，发送最新结果及有效期内最新的非正常结果（去重，最多两条），不从调度容错后的 `healthy/available` 推断。Router 经受认证的控制 RPC 接收，幂等存入独立展示表，保留七天，不回填半小时槽。旧 Router 忽略这两个根字段；旧 Server 缺字段时新 Router 回退周期结果，不把旧摘要当作可信手动证据。Share descriptor 及其签名结构不增加字段。

`GET /v1/shares/:share_id/model-health-calendar?days=N` 返回 UTC 日历聚合,`N` 被限制在 1..400,Dashboard 默认读取 365 天。每个 active 日同时返回 eligible、completed、observed、successful、已确认上游侧失败、monitoring gap、成功率和覆盖率；成功率固定为 `successful / eligible`,覆盖率为 `observed / eligible`,缺失检查不能算成功。历史槽、canonical observation 与已结束 epoch 保留 400 天。日历只对以下调用方可见:活跃公开 Share Market listing 的访客、Share Owner、管理员、活跃 ShareTo 用户。

Client 页和 Share Market 页的 Share 侧边栏使用同一个日历组件:日期方块按红、黄、浅绿、绿表达上游模型可用率,并显示月份横轴和星期纵轴。Tooltip 必须同时说明已观测数、确认的上游侧失败、监测缺口、覆盖率和同日配置切换；共享结果只标记“共享上游探针”,不披露其他 Share 数量。这一图表不能混用 30 秒连通率。

### 8.2 连接示例与手动测试

“连接”弹窗把手动操作明确标为**端到端连接测试**,对每个已启用 App 只显示一条 Server-authoritative 模型请求。Curl 的 method、path、body、流式 header 和测试模型全部由对应 `appRuntimes.<app>.modelProbe` 生成；Router 的手动 `test-connection` 经公开 Share 路由执行这份 probe 并按 `responseMode` 校验 JSON 或 SSE 完成事件。半小时定时探针则由 Server 在内部对绑定 Provider 执行同一测试模型。两者共用模型/策略来源,但证据范围不同,前端和 Router 后端不得保留硬编码的 GPT、Claude 或 Gemini 测试模型表。

完整成功的文字连接测试可记为 `router_connection_test` 实时展示证据，慢响应阈值使用 Server 经控制面提供的配置；旧 Server 未提供阈值时，成功结果不提升实时健康状态。测试前后须核对配置 revision、epoch、绑定和健康指纹，配置或阈值已变化则不记入展示证据。只有当前代表 App 影响概览颜色，其他接口成功不证明代表接口正常。失败不按 HTTP 状态直接归罪 Provider：用户自身余额、授权、配额、并发及控制链路错误不写全局健康失败；仅明确归属于当前 Provider 的账号限流可写失败。媒体测试和外部“本地测试”不写文字模型健康。健康记录为 best-effort，不得阻断连接测试或把已完成推理变成失败。测试完成后刷新 Dashboard，不伪造前端成功颜色。

Responses 文字流测试仅在完整终态 JSON 证明 `status=incomplete`、`reason=max_output_tokens`、整数 `usage.output_tokens` 达到本次实际请求的正整数输出预算时，接受 `response.incomplete` 为连接成功；保留真实 `terminalEvent`。必须读完整条流，HTTP 错误、协议错误、读取失败、取消或后续失败均优先于成功。`[DONE]` 本身不能证明成功，也不要求 Responses 流包含它。该规则只用于手动连接测试，不修改生产代理的终态。

模型策略提示同样读取 Provider 投影:`passthrough` 明确说明模型名会透传到具体供应商,调用方应使用该供应商支持的模型名；`single` 明确说明任意请求模型都会映射到固定上游模型。弹窗展示的“测试模型”、Curl 以及半小时周期使用同一个 Server 配置来源。

Grok Share 的手动测试另支持 `image_generation`、`image_edit`、`video_generation`。媒体操作只允许 Codex binding，只在 `grokMediaPolicy` 对应权限开启时展示和执行；Router 使用固定安全 prompt、内置合法 1×1 PNG 和固定 6s/720p/16:9 视频参数。Router 发出的 dashboard 测试只有在 loopback peer、精确内部 User-Agent 和内部 marker 三项同时满足时才把签名 `IngressContext.isHealthCheck` 设为 `true`；公网同名 header 会被剥离，不能伪造 HealthProbe。

### 8.3 Share 请求日志 usage 生命周期

Server 通过签名接口 `POST /v1/share-request-logs/batch-sync` 上送同一 `requestId` 的创建和终态记录。Token 数字字段为兼容字段，是否已经观测到真实 usage 必须由以下字段判定：

| 字段 | 语义 |
|---|---|
| `usageState` | `pending`、`observed`、`missing`、`parse_error`、`interrupted` 或媒体终态 `not_applicable` |
| `streamStatus` | 上游流终态或失败原因，例如 `completed`、`client_cancelled`、`timeout` |
| `usageRevision` | 同一请求内单调递增的 revision |
| `requestKind` / `operation` | `text|image|video` 及具体入口，例如 `responses`、`image_edit`、`video_status` |
| `parentRequestId` | 视频状态查询关联创建请求；历史任务可为空 |
| 媒体字段 | `mediaTaskId`、`mediaStatus`、视频 duration/resolution/aspect ratio 和有界 `errorMessage` |

- `observed` 允许所有 token 字段都为 `0`；这表示上游明确返回零，不能等同于 unknown。
- `pending`、`missing`、`parse_error` 和 `interrupted` 的数字兼容字段即使为 `0`，Dashboard、ticker、吞吐量和统计采样也不得当作真实零 usage。
- `share_request_logs` upsert 只接受 `excluded.usage_revision >= share_request_logs.usage_revision`，因此迟到或重放的低 revision pending 不能覆盖更高 revision 终态。
- 旧 Server 未发送这些字段时，Router 按 `usageState=observed`、`usageRevision=0` 兼容；新 Server 必须发送明确状态。
- `not_applicable` 不代表 token 为零；Image/Video 卡片不得渲染 token grid。`GET /v1/shares/:id/request-logs?requestKind=text|image|video` 在分页前过滤，并排除 `isHealthCheck=true` 记录。Dashboard 侧边栏以三个 Tab 读取同一接口；Image Tab 可用同一个 canonical `requestId` 关联旧图片结果表中的受控预览 URL。
- `GET /v1/shares/:id/recent-errors` 是公开只读接口，返回该 Share 最近 3 条已提交 HTTP 状态的非 2xx 快照（文本/图片共用槽位）。响应体按原样保存，截断仅发生在 8192 字节并追加 `...`。Owner/admin 看到完整 caller email；匿名与非 owner 看到 `mask_email` 后的邮箱。本接口不返回请求体、Authorization 或 IP，也不改写 `share_request_logs`。

**该命名空间对公网封闭**:`/_share-router/*` 的入站 GET 必须携带合法控制签名,否则返回 404(`src/proxy.rs:4032-4043`)。`/_ctl/*` 在所有公网入口点一律 404,不经路由(`src/proxy.rs:1459, 1845, 2165`)。

---

## 9. 用户统一模型入口

每个 Router 区域保留 `api.<tunnel_domain>` 作为所有用户共用的可选推理入口。它与 `<share-subdomain>.<tunnel_domain>` 直连并存，不创建第三种 tunnel，也不改变 Server 协议。`api` 是 namespace 保留标签，不能被 Client / Share claim。

### 9.1 配置控制面

`GET /v1/me/model-routing` 与 `PUT /v1/me/model-routing` 只接受 Router 用户 Session，不接受普通 API Key 代替浏览器 Session。响应包含：

| 字段 | 语义 |
|---|---|
| `apiBaseUrl` | 当前区域统一入口，例如 `https://api.example.com` |
| `enabled` | 当前用户是否至少配置一条 route；它不是区域 host 的部署开关 |
| `revision` / `updatedAt` | 用户 profile 的乐观并发版本与最后修改时间 |
| `routes[]` | 当前映射（精确或 `*` 全量）及稳定 route ID / 时间戳 |
| `eligibleShares[]` | 当前用户可选的 Owner、有效 ShareTo 或 Free Share，以及其已开启 App、直连 URL 和在线状态 |

`PUT` body 是 `{ expectedRevision, routes }`，整组 routes 在一个 Immediate 事务中原子替换；revision 不匹配返回 `409 model_routing_revision_conflict`，不会部分保存。最多 100 条。映射键固定为 `(appType, requestedModel)`，其中 `appType` 只允许 `claude | codex | gemini`，模型名 trim 后长度为 1–200、禁止控制字符并按 Unicode 字符串精确区分大小写。同一键只能指向一个 Share。

`requestedModel` 另接受保留值 `*`，表示该 `appType` 下**用户显式声明的全量路由**：任何未被精确映射命中的模型名都转发到该 Share。每个 `appType` 至多一条 `*`（由 `(appType, requestedModel)` 唯一键天然保证），`*` 与精确映射共用同一份 100 条上限。除恰好等于 `*` 外，`requestedModel` 不得包含 `*` 字符——`gpt-*`、`*-turbo`、`a*b` 一律返回 `400 user_model_route_model_pattern_unsupported`。Router 不提供前缀、后缀、通配段或正则匹配，`*` 是全量路由的唯一表示法。每次有效替换保存完整审计快照，并按用户只保留最近 100 个 revision，避免控制面反复保存造成无界存储增长。

创建或改变目标时，Router 要求调用方当前对 Share 具备 Owner、active 且未过期的 canonical `role=shareto` grant，或 Share 已开启 `freeAccess`，同时目标必须绑定并开启所选 App。原封不动保留的旧映射允许随整组草稿保存，即使其 Share 已删除、权限已撤销或 App 已关闭；这是为了让用户删除或修复其他映射，不代表失效目标仍可调用。Share 删除不会级联删除映射。

### 9.2 推理与模型列表

统一 host 接受以下用户 API Key 形式，三者按 `Authorization`、`x-api-key`、`x-goog-api-key` 的顺序解析，Token 必须带 `share:invoke` scope：

```text
Authorization: Bearer <router-user-api-key>
x-api-key: <router-user-api-key>
x-goog-api-key: <router-user-api-key>
```

公开 `GET/HEAD /v1/healthz` 不要求 Key。除 CORS `OPTIONS` 外，其余统一入口能力必须先通过 API Key 鉴权：

Share 直连 Host 的公开模型 API 与模型列表也提供无 Cookie 的 Bearer CORS；合法 OPTIONS 预检不要求 Key、不调用上游、不计量、不计入无效鉴权。实际请求仍执行原有 API Key、Share ACL、配额与计费检查，成功和错误响应均带跨域头。该能力不覆盖 Router 管理接口、`/_ctl/**`、`/_share-router/**` 或 Client Web。连接弹窗经用户确认（可按账号和目标站点记住选择）后，用 URL fragment 把 Share Chat Completions 地址、当前用户 Key 和测试模型传给外部本地测试站；配套工具以独立临时会话导入，不自动保存或调用模型，历史凭证脱敏。安全边界和两端发布顺序见 `docs/share-local-test.md`。

| App | 统一 Host 允许的完整推理路径白名单 | requested model 来源 | 模型列表 |
|---|---|---|---|
| Claude | `POST /v1/messages`、`POST /v1/messages/count_tokens` | JSON body 的非空字符串 `model` | `GET /v1/models` 且带 `anthropic-version` |
| Codex/OpenAI | `POST /v1/responses`、`POST /v1/chat/completions`、`POST /v1/completions`、`POST /v1/images/generations`、`POST /v1/embeddings` | JSON body 的非空字符串 `model` | `GET /v1/models`，不带 `anthropic-version` |
| Gemini | `POST /v1beta/models/:model:{action}`、`POST /gemini/v1beta/models/:model:{action}`、`POST /v1/models/:model:{action}`；`action` 仅允许 `generateContent`、`streamGenerateContent`、`countTokens`、`embedContent`、`batchEmbedContents` | URL 中 `/models/` 后、动作冒号前的模型段做 UTF-8 percent decode 后得到的非空值 | `GET /v1beta/models` 或 `/gemini/v1beta/models` |

上述表格是穷举白名单，不是示例或前缀匹配规则。即使某条路径可在 Share 直连入口使用，只要没有列在这里，也不能经统一 Host 转发。

白名单的收录标准是**能否确定性地提取出模型名**，因为统一 Host 靠模型名选路——提取不到就无从决定转发给哪个 Share。据此：

- 推理与其辅助端点只要请求体带必填 `model`（如 `count_tokens`、`embeddings`），就应当收录，它们在直连入口可用、在统一入口也必须可用；
- 模型名可选或根本不存在的端点（`multipart` 音频与图像编辑、`model` 可省略的审核端点）不收录。强行收录只会把直连能成功的请求变成统一入口的 400，反而扩大而非缩小两个入口的差距；
- Share 的 Web 路径与 `/_share-router/**` 控制面**永不**收录。它们不是推理端点，且统一 Host 是纯 API Key 入口，不得从这里暴露控制面。

Router 先从协议路径确定 App，再用 `(user_id, app, requested model)` 在单次查询内按固定优先级取唯一目标：

1. `requested_model` 精确相等的映射；
2. 该 App 下用户显式配置的 `*` 全量路由。

优先级是静态的、一次查询即定终局的确定性选路，不是失败后的回退：第 1 级命中时第 2 级不参与，第 1 级未命中时也不存在“先试再退”的过程。两级都没有映射直接失败。

无论命中哪一级，转发给 Share 的模型名**恒等于**客户端请求的模型名——Router 不改写请求体中的 `model`，由目标 Share 自行决定固定上游或继续透传。除用户显式写入的 `*` 记录外，绝不做模型改写、系统推断的默认 Share、按在线状态选 Share、前缀/后缀/正则/大小写不敏感匹配。`*` 命中的请求与精确命中走完全相同的 Share 存在性、App 开启状态、用户 ACL 与活动路由校验，同样 fail closed。找到映射后仍在当前数据库快照中重新检查 Share 存在性、App 开启状态和用户 ACL，再确认相同 Share ID 的活动内存路由，最后把请求交给既有 `proxy_handler`。后者会再次执行 Share edge ACL、并发、请求体限制、IngressContext、流式生命周期和响应清洗。

**候选切换（failover）**——每条路由可携带用户显式配置的有序候选目标列表（主目标为优先级 0，候选为优先级 1..=n）。转发时按优先级升序尝试：当前候选连接失败、超时或返回可重试状态码（401/403/429/5xx）时，自动切换到下一个候选。一旦任何响应字节已发给客户端，该次请求即锁死在当前候选，绝不切换（避免客户端收到拼接的两段响应）。候选耗尽时报错，fail closed——绝不臆造列表之外的目标。每个候选目标配有进程内存态熔断器（连续失败 3 次开启，冷却 60s 后半开单路探测），熔断仅由真实转发流量驱动，与 Share 健康探测解耦。候选列表是用户显式声明的，可见、可审计、可删除，不属于"系统推断的默认 Share"。

模型列表分两种模式，取决于该用户在该 App 下的路由配置形态。

**透传模式**——当该 App 下**有且仅有**一条 `*` 全量路由（没有任何精确路由），且该目标 Share 通过与上一段**完全相同**的实时校验时，这个入口在该 App 上就是目标 Share 的纯转发层。此时模型列表请求按推理请求同样的方式改写 Host 并交给 `proxy_handler`，由目标 Share 返回其真实 catalog，Router 不解析、不重排、不合成 envelope。客户端因此拿到与直连该 Share 完全一致的响应，包括供应商自有字段。目标 Share 离线或隧道重连时，返回的错误也与直连该 Share 时一致。

**合成模式**——其余所有情况（存在精确路由，或没有 `*` 路由，或 `*` 目标未通过实时校验）。此时没有任何单一上游能代表这个入口，Router 使用供应商兼容 envelope，按下列规则枚举并去重后按名称升序：

1. 该用户为对应 App 配置的**精确** requested model 名称；
2. 若该 App 存在 `*` 全量路由且其目标通过实时校验，追加该 Share 在模型健康探测中记录过的模型名。

保留值 `*` **绝不出现在模型列表中**——它不是可调用的模型名，客户端不得将其选中。第 1 类是用户自己写下的字符串，不泄漏任何信息；第 2 类属于目标 Share，因此撤权、关闭该 App 或删除 Share 后必须立即从列表中消失。第 2 类同时是尽力而为：目标从未被探测时该部分为空，列表可能只含第 1 类甚至为空，这不是错误，也不影响 `*` 路由对任意模型名的实际转发能力。合成模式下的列表不汇总 Share 的完整上游 catalog，也不代表目标此刻在线。

两种模式下真正调用都始终按上一段重新校验并 fail closed。模式的选择只取决于路由配置形态，不取决于目标此刻是否在线——透传模式不会因为目标离线就退回合成模式，否则同一份配置会因为上游抖动而返回两种语义不同的列表。

选中 Share 后，请求与直连走同一 Share 数据路径。Server 和日志不获得独立的“统一入口资源”身份；请求记录、usage、并发、地图归因、模型健康、限额及市场账务仍落在目标 Share。首页地图和 Share 侧边栏因此保持 Share 粒度。

### 9.3 错误、Host 隔离与 CORS

路由选择层的错误使用稳定 JSON envelope：顶层 `message` / `code` / `details`，并同时提供 OpenAI 兼容的 `error.message` / `error.type` / `error.code`。响应带 `x-share-router-error: true`。目标 Share 一旦选中，真实上游响应和既有 Share 本地错误继续按第 10.2 节原样返回，不按状态码重写。

| HTTP | 稳定 code | 条件 |
|---:|---|---|
| 401 | `user_api_token_invalid` | Key 缺失、失效或 scope 不足 |
| 403 | `user_model_route_client_banned` | 调用 IP 已被 Router 的短期滥用防护封禁；按 `Retry-After` 重试 |
| 404 | `user_model_route_path_unsupported` | 统一 host 上的非白名单路径 |
| 405 | `user_model_route_method_not_allowed` | 推理路径不是 POST |
| 408 | `user_model_route_request_body_timeout` | 请求体读取超时 |
| 413 | `user_model_route_request_body_too_large` | 命中 Router 对应请求体上限 |
| 422 | `user_model_route_model_required` | 无法取得非空 requested model |
| 400 | `user_model_route_model_pattern_unsupported` | 配置的模型名含 `*` 但不等于 `*`；Router 不支持模式匹配 |
| 404 | `user_model_route_not_configured` | 精确键与该 App 的 `*` 全量路由都没有映射 |
| 403 | `user_model_route_unauthorized` | 映射目标的用户权限已失效 |
| 409 | `user_model_route_app_unavailable` | 目标不再绑定或开启该 App |
| 503 | `user_model_route_target_unavailable` | Share 记录、subdomain 或活动路由不可用 |

Host 判断必须精确匹配 `api.<tunnel_domain>`（含配置中的端口语义），`nested.api...`、后缀拼接域名或不同端口都不能进入该分支。该 host 不提供 Dashboard、Session 控制面、Share Web、Client Web、`/_ctl/*` 或 `/_share-router/*`；未知路径不得落回静态 UI/catch-all。

所有统一入口实际响应带 `Access-Control-Allow-Origin: *`、`Cache-Control: no-store` 并暴露公开 request/error headers。`OPTIONS` 无需 Key，返回 `204`、`GET, POST, OPTIONS`，并回显浏览器请求的 `Access-Control-Request-Headers`；不启用 credentialed cookies。DNS/证书必须覆盖 `api.<tunnel_domain>`，部署约束见 [README.md](README.md)。

---

## 10. 身份注入:IngressContext

Router 转发到 Server 的每个请求都会剥离客户端可伪造的凭据头(`authorization`、`x-api-key`、`cookie`,见 `src/proxy.rs:2580-2596`),改为注入一个签名的身份上下文。

结构见 `src/ingress_context.rs`:`protocolEpoch`、`routerId`、`routeId`、`installationId`、`targetLaneId`、`publicHost`、`shareId`、`requestId`、`userEmail`、`userRole`(`owner` / `admin`)、`userCountry`、`issuedAtMs`。

签名以 `control_secret` 为密钥,签名域 `cc-switch-router-ingress-v1`(`src/ingress_context.rs:11, 50`)。

Server 侧必须:

1. 校验签名
2. **校验 `issuedAtMs` 新鲜度**:允许 `server_now - issuedAtMs <= 30000ms`,同时只允许 `issuedAtMs - server_now <= 5000ms`;两个边界值本身有效
3. 剥离客户端自带的所有 `x-cc-switch-*` 头,再由已验证的上下文重新注入

> Router 侧 `sign` 只校验 `issued_at_ms > 0`(`src/ingress_context.rs:72`),**不做接收侧重放窗口检查**。新鲜度判定完全依赖 Server 实现;若 Server 放宽该窗口,历史签名上下文可被重放。

这个窗口要求 Router 和 Server 主机都保持可信 UTC 时间,但不能通过简单扩大窗口掩盖校时故障。Router 使用外部 HTTPS Date 仲裁观测主机偏差:本机慢 15/25 秒分别告警 warning/critical,本机快 2/4 秒分别告警 warning/critical,为接收边界保留余量。Router 进程不负责改系统时间。

Server 拒绝带 ingress 的请求时仍返回空正文 `401`,并仅向 Router 附加以下内部诊断头:

| Header | 内容 |
|---|---|
| `x-cc-switch-internal-ingress-error` | 稳定原因码；时间类为 `expired` 或 `future_timestamp`,其余为签名、身份、epoch、字段或配置契约错误 |
| `x-cc-switch-internal-ingress-age-ms` | 仅时间类错误存在,值为 `server_now - issuedAtMs`,可以为负数 |
| `x-cc-switch-internal-ingress-server-time-ms` | 仅时间类错误存在,Server 校验时的 Unix 毫秒时间 |

Router 对所有上游响应无条件剥离 `x-cc-switch-internal-*`。带 typed freshness 原因的 `401` 映射为 `503 ingress-clock-skew` 并附 `Retry-After: 5`;其他 typed ingress `401` 映射为 `502 ingress-contract-rejected`;没有 typed 头的普通业务 `401` 保持不变。不得根据空正文或 URL 猜测时钟偏差。

滚动发布必须先部署剥离/识别这些内部头的 Router,再部署发送诊断头的 Server；回滚先 Server 后 Router。

### 10.1 请求体上限声明:`x-cc-switch-ingress-body-limit`

与签名上下文同批注入,但**不参与签名**:值是十进制字节数,等于 Router 为本次请求命中的档位上限(普通 / 视频 / 图片,见 `src/proxy.rs` `proxy_request_body_limit()`)。

Server 侧契约:生效上限取 `min(本地上限, 声明值)`。因此该头无需签名——伪造只能把上限压低(伪造者自伤),抬不高 Server 的本地配置;Router 还会通过 `is_internal_share_context_header()` 剥离来自公网的同名头,Server 看到的值只可能由 Router 写入。

两端可独立升级,任意顺序:

- 旧 Server 不认识该头 → 沿用自身硬编码上限,行为不变。
- 旧 Router 不发送该头 → 新 Server 回退到历史默认值(普通 2 MiB / 视频 32 MiB / 图片 48 MiB)。

Server 的本地上限默认取 Router 允许的最大档位,使 Router settings 成为默认的唯一天花板;Server owner 可用 `requestBodyLimits` / `CC_SWITCH_{,MEDIA_,IMAGE_}REQUEST_BODY_LIMIT_MB` 主动收紧。

### 10.2 推理身份与本地并发错误

- Direct Share 与统一模型入口都使用 Router API Token 解析出的规范化邮箱作为 `IngressContext.userEmail`；统一入口选中目标后不创造另一种终端用户身份。
- Gateway 请求不会伪造终端用户邮箱；Router 仅注入 `dataSource=gateway`、Share/route/request identity 和可信地域字段。不得把 Gateway owner email 当成终端用户身份，也不得只在 Router 做授权后向 Server 签发空用户身份。
- Gateway 不伪造终端用户邮箱。非免费 Share 缺少 `userEmail` 时由 Server fail closed,返回 `cc_switch_user_identity_required`；`/_share-router/*` 健康探测不进入 Share 用户授权与并发校验。
- Router 必须剥离调用方提供的 `x-user-email` / `x-user-country*` 等旧身份头；Server 推理上下文只接受签名上下文重新注入的 `x-cc-switch-user-*` 头。
- Email grant 的 `parallelLimit` 由 Server 统一执行,并在同一 Share 下跨 Claude、Codex、Gemini Surface 共用。Router 的 email inflight 仅用于观测,不得成为第二套授权或限额权威。

本地并发槽位冲突使用 `409 Conflict`,不复用 `429 Too Many Requests`。`429` 仅表示上游限流、RPM/TPM 或时间窗口配额。槽位释放时间未知,并发冲突不得发送虚假的 `Retry-After`。

| 稳定错误码 | scope | 执行方 |
|---|---|---|
| `cc_switch_user_concurrency_limit_exceeded` | `user` | Server email grant |
| `cc_switch_share_concurrency_limit_exceeded` | `share` | Router / Server Share 池 |
| `cc_switch_provider_account_concurrency_limit_exceeded` | `provider_account` | Server Provider account |
| `cc_switch_free_share_ip_concurrency_limit_exceeded` | `free_share_ip` | Router 免费 Share IP 池 |
| `cc_switch_image_concurrency_limit_exceeded` | `image` | Router 图片任务池 |

所有本地推理错误返回 `x-cc-switch-error-code`、可用时返回 `x-cc-switch-error-scope`，并同时返回 `x-cc-switch-request-id` 与 `x-request-id`。OpenAI/Codex 使用 `error.message/type/code/param/details`；Anthropic 使用 `type=error`、`error.type/message/code/details` 和 `request_id`,本地并发错误附 `x-should-retry: false`；Gemini 使用 HTTP `409`、RPC `ABORTED` 与 `google.rpc.ErrorInfo`。并发详情只公开锁内捕获的 `current/limit`,不得包含用户邮箱。

Router 原样转发 Server 的公开错误头与响应体,不得按状态码重写真实上游错误。指标只在稳定本地错误码匹配时记为 `concurrency_limited`;普通 `409` 仍按上游错误处理。

---

## 11. 边界策略

Client web 隧道上的路径可达性(`src/proxy.rs`):

| 路径 | 策略 |
|---|---|
| 静态资源、登录 / OAuth 回调 | 公开 |
| `/web-api/*` 其余路径 | 要求 owner / admin 身份(`is_client_web_auth_required_path`,`proxy.rs:4179`) |
| `/api/*`、`/v1/*` | **不经 client web 隧道暴露**(`is_allowed_client_web_path`,`proxy.rs:4167-4176`) |
| `/_ctl/*` | 公网一律 404 |
| `/_share-router/*` | 需控制签名 |

Share 直连隧道另有独立白名单:`/v1`、`/v1/`、`/v1beta/`、`/gemini/v1beta/`、`/_share-router/`(`is_allowed_direct_share_api_path`,`proxy.rs:4148`),并要求带 `share:invoke` scope 的 Router API token。

流式管理接口必须使用 `Authorization` 头,不接受 query-string token。

---

## 12. 协议严格性

- **Epoch 不匹配硬失败**,不降级
- Client installation 注册只接受 `register_installation` 规范串;auth device 注册只接受 `register_auth_device` 规范串
- 注册字段全部必填且拒绝未知字段,不存在注册 proof 版本字段、无签名注册或旧签名串分支
- Client installation 注册响应必须包含 `controlSecret`;Server 不接受缺少该字段的成功响应
- Share descriptor 同步:Server 优先调用 `POST /v1/shares/descriptor-batch-sync`;收到 404 时回落至 `POST /v1/shares/batch-sync`,并剥离 `descriptorGeneration` / `descriptorFingerprint` 字段。两条路径都对请求体 `ops` 原文验签;`POST /v1/shares/sync` 对 `share` 原文验签
- `POST /v1/shares/claim-subdomain` 的新请求签 `claim` 原文；`claim.shareSha256` 必须是同一请求中 `share` 原文的 lowercase SHA-256。Router 暂时兼容没有 digest 的旧 claim，以及直接签 `share` 原文的更旧请求
- `POST /v1/shares/edit-ack` 对 `{"ack":<请求体 ack 原文>}` 验签，包含 `ack.currentShare` 时不得重序列化 descriptor
- 上述 Share sync/claim/edit-ack 控制请求为兼容旧 Server 可省略 `protocolEpoch`；一旦出现就必须是当前 epoch 的非空字符串，`null` 与错误 epoch 都拒绝
- Share descriptor、其嵌套权威对象和 batch operation 都拒绝未知字段；契约扩展必须提升 `contractVersion` 并先部署 Router。Server 的 `shareSha256` 请求必须同样遵循 Router-first 发布顺序

## 13. Share v10 美元等值额度

此功能与钱包、充值、市场租金完全独立。完整实现规格和真实灰度门槛见 [实施计划](docs/share-user-value-quota-implementation-plan.md)。当前只声明本地／夹具验证；真实转发面保持 `live_pending`。

### 13.1 不可变执行价目（由模型价格自动派生）

执行价目没有单独的编辑或发布接口：它就是「设置 → 市场与计费 → 模型价格」页的价目（LiteLLM 派生行 + 管理员在该表为未定价模型或覆盖填写的价格），在某一时刻冻结成快照。Router 在启动及每次价格变更（`reload_pricing_catalog`）后派生快照；内容修订变化时写入并发布——首个价目立即生效，替换已有价目时生效时间为 now+60s，便于已连接的 Server 先安装和 ACK。同一内容修订的首次生效时间不可变。派生规则与价格显示一致：每个模型按显示所用回退链（同 tier 的 base → standard 同上下文 → standard/base）物化全部 `standard|priority|flex` × `base`（及有阈值时的 `long`）行；`contextInputBasis=rawInput`（与显示一样计入缓存输入判定长上下文）；缺 1h 缓存写价时 1h 与 5m 同价、`cacheWritePolicy=both`；单个非法模型行跳过并记日志，不影响其他模型。被替换超过 30 天的修订其 `payload_json` 压缩为 `{}`，行保留以满足 ACK 和 Share 绑定引用。

快照 schema v1 使用 `currency=USD`、`tariffPolicyId=api-equivalent-v1`、准确 `models` 和显式 `exactAliases`。费率是整数微美元／百万 Token 的十进制字符串；`null` 为不可用，不是零价。模型和别名唯一；最多 10,000 模型、40,000 别名、4 MiB（当前完整 LiteLLM 目录约 1.9 MiB）。修订为规范字段顺序及排序 map 序列化的 SHA-256；测试向量见 `tests/fixtures/share-value-pricing-v1.json`。模型解析与显示相同：先原样，再按显示的身份剥离（`anthropic.` 等厂商前缀、`publishers/…/models/` 路径、`-thinking`／`-latest`／`[1m]`／Bedrock `-v1:0`、日期戳、`claude-sonnet-4.8` 写法）逐个候选匹配精确别名或模型键。未定价模型不可被美元额度用户调用，需在模型价格表定价。本节接口请求体上限 256 KiB；价目只在 fetch 响应中由 Router 下发。

Server 使用通用 installation Ed25519 envelope（`protocolEpoch/installationId/timestampMs/nonce/signature`，同 §3 签名串）调用：

| 路径 | action | 签名 payload |
| --- | --- | --- |
| `POST /v1/shares/value-pricing/fetch` | `share_value_pricing_fetch` | `{}` 或 `{knownRevision}` |
| `POST /v1/shares/value-pricing/ack` | `share_value_pricing_ack` | `{revision,storageSchema:3}` |
| `POST /v1/shares/value-pricing/active` | `share_value_pricing_active` | `{revision,storageSchema:3}`；需协商 `valuePricingActivationV1` |
| `POST /v1/shares/:share_id/value-quota-watermarks` | `share_value_quota_watermark` | `{shareId,status}` |

**签名 payload 规范化**：上述四个 action 的 payload 是请求体中除 envelope 五个字段外的全部字段，签名串中的 `<payload_json>` 必须是其**规范 JSON**：所有层级对象的键按 UTF-8 字节序排序、无任何空白、数字与字符串按 serde_json 默认格式输出。Router 将收到的 payload 解析为 JSON 值后重新序列化验签（Router 的 serde_json 未开启 `preserve_order`，输出即为排序形式），因此请求体线上的键顺序和空白无关，但 Server 必须对规范形式签名。双仓共用夹具 `tests/fixtures/share-value-signed-payload-v1.json`（`canonical` 字段）。Router 先在不持有写锁的情况下读取 installation 公钥并完成时间戳、Ed25519 验签与 payload 校验，再在 IMMEDIATE 事务中复核公钥、消费 nonce 并执行；nonce 防重放语义不变，验签或校验失败不消耗 nonce。

fetch payload 只允许 `{}` 或 `{"knownRevision":"<rev>"}`，其他键 400。Router 的发布修订是全局唯一的（不按 installation 区分），ACK 按 `(installation, revision)` 记录。`knownRevision` 等于当前发布修订**且**本 installation 已有该修订的 ACK 行时，只返回 `{"unchanged":true,"revision":"<rev>"}`，不构造、不序列化快照；本 installation 没有 ACK 行时返回完整清单（与无 `knownRevision` 时相同），以便 Server 重新安装并重新 ACK。旧 Router 对 `knownRevision` 返回 400 时，Server 回退为 `{}` 重新 fetch。ACK 幂等：同一 `(installation, revision)` 重复 ACK 返回成功，但保留首次 `acknowledgedAtMs`。

fetch 返回 null 或 `{installationId,revision,effectiveFromMs,snapshot}`。Server 校验绑定、大小及散列后立即安装到 committed SQLite 并 ACK，即使 `effectiveFromMs` 在未来；未来生效的价目由 Server 在 `effectiveFrom` 到达时激活，此前继续使用已激活的旧修订。无快照时不开启美元模式。断连时继续使用最后确认的本地快照，不能接受无法核验的新修订。调价只作用于新准入请求；历史使用量保留冻结修订和费率。

到期激活不依赖下一次 heartbeat：Server 每秒本地检查一次；准入 IMMEDIATE 事务再次检查到期排期，并在同一事务选择／冻结执行修订。重启、断连和并发激活均从本地 committed SQLite 收敛；已有 attempt 的幂等重放仍用原冻结修订。ACK 只证明安装成功，不证明正在执行。Router heartbeat 增量声明 `valuePricingKnownRevisionV1` 和 `valuePricingActivationV1`；协商后一并上报当前实际 active revision。Router 只接受本 installation 已 ACK 且 Router 时钟已过生效时间的 signed active 回执，按签名时间拒绝旧报告覆盖新证据。旧 Server 不发回执时只显示“激活未确认”，不阻止已有有效快照运行；旧 Router 不支持该能力时 Server 不调用新端点。部署顺序为 Router → Server。

Router 每 15 秒重试执行价目发布失败，并检查目录内未来价格代际是否到期；保留原 60 秒缓冲，不假定“保存成功”即“Server 生效”。价格保存响应附 `executionTariff={state,revision,effectiveFromMs}`，`state=pending|published|unknown`。迁移 0106 仅新增发布进度及 activation receipt 表，不改钱包、金额配额、历史 ACK 或计费记录。

健康接口增加 `serverTimeMs`、`publication.state` 及 `delivery={expectedInstallations,acknowledgedInstallations,activatedInstallations,unconfirmedInstallations}`。统计对象为有 Share 的 active installations；只有回执匹配最新发布修订、回执和 heartbeat 均在 180 秒内才计为已激活。无回执、离线或过期均不推断已激活。模型价格页自动轮询（页面隐藏时停止、账号切换清除、次数有界并可手动重查），仅依据 Router 时钟区分待生效／待 ACK／待确认／部分及全部激活。

### 13.2 精度、准入和动态水位

预览与用户主动保存 USD 策略增加 `model_price_coverage`：对启用且已验证的 Single 模型绑定，检查实际 upstream model 在当前激活快照的 standard tier 中可定价；排期含价但未生效时返回 `reasonCode=tariff_not_effective`。Passthrough 无法静态枚举，标 `scope=request_time`，每个请求仍检查实际模型与 tier。缺价拒绝新的 USD 编辑，不阻断与策略无关的后台保存，不重置原授权。预览最多推进已到期的本地价目元数据，不写配额／用量。

发送前的 `price_unavailable` 错误附 `reasonCode=model_unpriced|tier_unpriced|tier_unknown|tariff_not_effective`、有界的 `model/tier`、`activePricingRevision/pendingPricingRevision/effectiveFromMs`。OpenAI／Anthropic 放在 `error.details`，Gemini 放在 `ErrorInfo.metadata`；未知字段可忽略。人类可读消息不重复错误码，也不含完整 payload／凭据。日志记录 request/share 与修订事实及 `upstream_sent=false`，不记录请求内容。

额度 `limitUsdMicros` 必须大于零且最多 `1000000000000000`。累计量使用整数 picos 字符串，`1 USD=10^12 picos`；每分类贡献直接为 `tokens × rateMicrosPer1m`，不先截断成美分。cache write 只有合并量时使用已证明的两档最高价并标记 `conservative`；没有可信上界则不可用。

Server 在真正发上游前以 IMMEDIATE SQLite 事务校验并写入风险占用。`spent + pendingRisk >= limit` 拒绝新请求，已准入请求自然结束，可超过额度，是请求级软上限而非绝对硬上限。每个请求的风险预占有界（不再预占全部剩余额度）；同一用户的金额模式并发为 `min(parallelLimit, 4)`（未设并发即 4），超出时返回 `value_quota_busy`。

**不再有 shadow 计量**：Token 模式授权不再获得 `valueMetering` 绑定，Server 也不为其记录金额。只有 `policy.valueQuota` 生效或携带 `pendingValueQuota` 的授权才有绑定；Router 在 descriptor 同步时丢弃其他授权上残留的旧绑定，且只对这些授权检查价目 ACK。缺少 ACK 时整份 descriptor 返回 409 `value_pricing_ack_missing`，`details.revisions` 列出（去重后）未 ACK 的修订；Server 对这些修订重新 ACK 后自动重试一次同步。作为兜底，Router 在 fetch 的 `knownRevision` 匹配但本 installation 无 ACK 行时返回完整清单（见 §13.1），使 Server 重新安装并 ACK。计量起点只在同为金额模式（启用或排期）的前后绑定之间不可变。

**启用规则**：自然周期（day/week/sevenDays/calendarMonth/thirtyDays）从下一个窗口开始执行，除非该用户当前窗口已经是金额计量，或该用户在当前窗口内尚无任何已记录请求（全新用户）——此时 Server 立即启用，`meteredFrom` 为当前窗口起点；`lifetime` 与 `accountReset` 立即开始，`meteredFrom=now`。

**按结果结算**：未发出上游请求，或上游错误且无 usage → $0；流式中断 → 已观测部分 usage 加按已流出文本估算的输出，标记 `conservative`；结果未知 → `max(已观测, 预占)`，标记 `conservative`。超时未结算的 pending 记录由 Server 的清扫任务按上述规则收敛，不会永久占用风险。

仅允许以下实际 profile／driver 组合的文本请求；预览、保存与发送前共用同一能力函数：

| Profile | Driver |
| --- | --- |
| `claude.anthropic_api_key` | `http.anthropic_messages` |
| `codex.openai_api_key` | `http.openai_responses` |
| `gemini.google_api_key` | `http.gemini_native` |
| `claude.custom_http`、`gemini.custom_http` | `http.anthropic_messages`、`http.openai_chat`、`http.openai_responses`、`http.gemini_native` |
| `codex.custom_http` | `http.anthropic_messages`、`http.openai_chat`、`http.openai_responses` |
| `claude.workbuddy_oauth`、`codex.workbuddy_oauth`、`gemini.workbuddy_oauth` | `special.workbuddy_oauth` |

不只信任兼容 ProviderType。上述自定义 HTTP 组合沿用 Registry 已实现的跨协议转换，计量使用转换前的原始上游模型、tier 和用量。WorkBuddy 在目录和认证准备完成、最终模型及 payload 确定后、首次生成发送前准入；`auto` 只能解析到可定价的具体模型，不透明 `default/default-model` 返回 `price_unavailable`（`reasonCode=model_unresolved`），不能以 WorkBuddy 积分代替美元价格。其 HTTP 401 免费重试须先同步结算前一 attempt，再以新 attempt 准入；已观测输出／用量的请求不得自动重放。响应缺失模型、跨帧冲突、未知 tier、截断和结果未知走保守结算，不把合成模型当上游证据。

其他 OAuth（包括共用实现的 CodeBuddy）、Web Session、Coding Plan、媒体、内置付费工具、未列出的组合和未知 retry 仍不可用；客户端 function tool 文本允许，Server 不执行工具。旧 Token 模式保持不变。新增组合仅作本地夹具验证，不宣称生产流量验收。

`status` 包含 `email/policyEpoch/windowId/windowStartsAtMs/resetsAtMs/meteredFromMs/activePricingRevision/usagePricingRevisionsDigest/usageWatermark/limitUsdMicros/observedPicos/manualOffsetPicos/usedPicos/pendingRiskPicos/meteringStatus/syncedAtMs`。窗口跨策略 epoch 聚合，不按 epoch 归零。`meteringStatus=observed|conservative|pending|blocked|scheduled`；`unavailable` 仅表示该行不可读。`scheduled` 仅在当前策略不是金额模式、且有待生效金额策略时出现（此时 `limitUsdMicros=null`）；金额策略正在执行时 Server 报告真实当前状态，即使另有待生效的新金额策略。待生效字段（`pending*`）始终由 Router 从已存 descriptor 补充，不在水位 payload 中。Router 对未知取值不拒绝水位，而是按 `unavailable` 存储并记 warn。Router 对每个 `(Share, 用户)` 只保留当前窗口一行：新窗口行写入后删除旧窗口及其他 installation 遗留行；状态接口只读取当前窗口。删除 Share 或清理 installation 时同时删除其投影与 ACK。Server 水位保活间隔 45 秒（心跳 60±6 秒）；Router 健康接口将 180 秒未确认的当前窗口投影计为 `staleGrants`。动态水位不更改静态配置、generation 或 fingerprint。Router 校验当前 installation/grant/epoch/窗口和价目 ACK，旧水位幂等返回较高已确认水位；相同水位不能改变金额，更高合法修订允许金额下降。Server 如收到超出本地真实水位的 ACK，冻结该 scope 并要求恢复核对。

失败、499、504 和取消仍计已观测使用量；流式未确认结束、缺量、模型不符等不能释放风险为免费。Server 代理错误码：

| code | HTTP | 含义 |
| --- | --- | --- |
| `value_quota_exhausted` | 403 | 已用 + 在途风险达到额度；带 `resetsAt`（lifetime 无） |
| `value_quota_busy` | 429 | 金额模式并发已满，带 `Retry-After` |
| `price_unavailable` | 503 | 当前执行模型／档位无有效价目 |
| `quota_unavailable` | 503 | 存储、覆盖、ACK 或计量能力不可用 |

控制面版本冲突使用 `quota_revision_conflict`（409）。都不等同钱包不足。

原生 Gemini 的执行模型取自真实 URL，而非客户端 JSON `model`。上游响应模型证据与用量原子落盘；流式分片中的矛盾模型不能被后续正常帧抹去。响应模型与执行模型不同，仅冻结快照明确声明的 exact alias 可证明同一身份，不使用同族／日期后缀猜测。异常响应保留风险并暂停该窗口的新美元请求，不能先释放风险、再异步补记异常。

Gemini 的金额模式仅覆盖 `generateContent`／`streamGenerateContent`；辅助 `countTokens`、embedding、batch 等未覆盖动作在金额预占和发网前返回 `quota_unavailable`，不改变旧 Token 模式的可用动作。

### 13.3 私有读取与校准

以下浏览器接口都要求登录并返回 `Cache-Control: private, no-store`：

| 接口 | 访问范围 |
| --- | --- |
| `GET /v1/shares/:share_id/value-quota-status` | Share Owner / Router Owner，当前授权用户投影 |
| `GET /v1/me/share-value-quotas?shareId=…` | 仅当前用户自己的投影；每行自包含 |
| `POST /v1/shares/:share_id/value-quota-preview` | Share Owner / Router Owner；`expectedConfigRevision` + `policies`，经 `/_ctl/preview_value_quota` 只读取得权威预览 |
| `POST /v1/shares/:share_id/value-usage-edits` | Share Owner / Router Owner；`expectedConfigRevision` + `edits`，经 `/_ctl/edit_value_usage` 执行独立金额校准 |
| `GET /v1/shares/:share_id/market-value-quota-readiness` | Share Owner / Router Owner；`{ready,valueTrialReady,checks}` 或 `{ready:false,reason,checks}`，与发布／编辑／恢复／报价美元车位时的同一就绪门槛；旧响应可省略 checks |
| `GET /v1/admin/share-value-pricing/health` | Router Owner；启用／待生效／遗留绑定（`shadowGrants`，仅统计尚未被新 descriptor 覆盖的旧 shadow 绑定）／延迟及价目 ACK 数，无用户邮箱 |

成功与错误响应（含 401/403/404 与提取器拒绝）一律带 `Cache-Control: private, no-store`。两个状态接口的 `items[]` 在 `status` 字段之外附加 `tokenPeriod`、`pendingLimitUsdMicros`、`pendingTokenPeriod`、`pendingEffectiveFromMs`（无排期时为 null），因此前端无需授权对象即可展示 `limitUsdMicros/usedPicos/pendingRiskPicos/windowStartsAtMs/resetsAtMs/meteredFromMs/meteringStatus/syncedAtMs` 与排期。当前策略非金额模式、只有待生效金额策略且尚无当前窗口投影的有效授权，返回仅由已存 descriptor 构造的最小行 `{email,meteringStatus:"scheduled",tokenPeriod,pendingLimitUsdMicros,pendingTokenPeriod,pendingEffectiveFromMs}`；用量、窗口、`limitUsdMicros` 与 `syncedAtMs` 字段缺省，绝不伪造 $0 用量。

预览增量字段：`capabilityReady` 表示基础设施能力，`ready` 同时要求所提交用户策略的历史覆盖可用；不能用单个用户的历史缺口禁用整个 Share 的单位选择。`checks[]` 逐项包含 `{code,ready}`：`router_protocol`、`sqlite_authority`（附 `state=committed|prepared|shadow_verified|unavailable`）、`metering_storage`、`acknowledged_tariff`、每个绑定各一项 `text_surface`（附 `app/profileId/driverId`）。Router 另加 `tariff_published` 及本 installation 的 `pricing={publishedRevision,effectiveFromMs,acknowledgedAtMs,version}`。失败检查可带可选 `blockedBy`（另一检查的 `code`），表示它只是该根因的后果：Server 在存储未提交时给 `acknowledged_tariff` 标 `sqlite_authority`，否则在协议未协商时标 `router_protocol`；Router 在未发布价目时给仍无 `blockedBy` 的失败 `acknowledged_tariff` 标 `tariff_published`。UI 只把无 `blockedBy`（或其阻塞项本身已就绪）的失败检查当作需处理的根因。该最新发布信息与 `activePricingRevision` 分开：新价待生效或待确认不作废旧的有效快照。能力不满足时返回全部检查和空 `rows`，不查询用户窗口；revision 冲突仍为 409。旧 Server 缺检查字段时 UI 保持关闭并提示升级。

控制连接失败与 Server 5xx 统一为安全 `quota_unavailable`（503）；预览与金额校准对 Share 离线／无 installation 绑定一致返回 `readinessCode=quota_server_offline`，缺少控制凭据或 Server 401/403 为 `quota_control_unauthorized`。`details.readinessCode` 区分 `quota_server_offline|quota_server_timeout|quota_server_upgrade_required|quota_control_unauthorized|quota_server_invalid_response|price_unavailable|quota_unavailable`。Server 的 4xx 客户端错误原状态透传，`code` 取白名单 `invalid_policy|quota_target_invalid|quota_revision_conflict|value_quota_not_active|value_quota_busy|quota_target_not_found`（未知码按状态回落：404→`quota_target_not_found`，409→`quota_revision_conflict`，429→`value_quota_busy`，其他→`invalid_policy`），`details.reasonCode` 取 Server 的 `details.reasonCode`（仅小写 token），缺省时无码 400 为 `invalid_policy`、其他为该 `code`；`quota_revision_conflict` 另带 `previewRequired:true`。带 JSON 错误体的 404（未知 Share／用户）透传为 404；空体的路由级 404/405 才视为旧 Server（`quota_server_upgrade_required`）。错误消息由 Router 生成，不转发上游正文、凭据或网络细节。预览响应须匹配请求的配置版本。配置、账号或 Share 变化后取消／忽略旧检查；后台检查不覆盖显式策略预览。

市场就绪诊断和普通预览共享 `checks`：`text_surface`／`model_price_coverage` 可带 `app/providerId/profileId/driverId/model/scope/reasonCode/effectiveFromMs`。`runtime_unsupported` 表示未覆盖执行路径；`model_unresolved` 表示模型仍为不透明别名，不应提示用户只改价格。固定模型提前验价；Passthrough 仅声明 `scope=request_time`。市场响应匹配 Share 配置 revision，checks 类型、数量和字段长度均校验，仅透传诊断字段白名单，不传凭据或上游正文；`reason` 仍保留兼容旧客户端，失败不可因通用成功标志被忽略。

金额校准为独立 `set|clear`，携带 grant revision、windowId、observed watermark、operationId 和审计原因；set 不得低于真实观测，clear 仅清人工偏移。批量全成或全败。金额校准永不返回 500：前置条件失败为 400/409（`quota_target_invalid`、`quota_revision_conflict`、`value_quota_not_active`），未知 Share／用户为 404，存储不可用为 503；成功响应 `items[]` 中不可读的行带 `error:{code}`，Router 原样转发。`set` 同时是对账（reconcile）操作：Server 先收敛已超过请求最大超时加余量（与 sweeper 同一阈值）仍未结算的 attempt（较新的在途 attempt 保持预占）、重建检查点，并清除该 scope 的阻断／restore fence；`expectedWatermark` 不再要求精确等于当前水位，幂等只依赖 `operationId`，且 `target >= 当前已观测`。旧 Token `usageRebase/userUsageEdits` 不承载金额；活跃市场 managed grant 仍只读。原公开用量 API 不附加私人金额策略；客户端切账号清理响应，超过 180 秒标注同步延迟，缺状态不显示伪造的零。

新增普通授权的**初始已用金额**与上述即时校准是不同操作。新版预览增加构建能力 `checks[]={code:"value_usage_initialization",ready:true}`；仅同时满足新用户、无任何历史用量／人工补录／恢复 fence、USD 策略当前立即生效时，该用户行返回 `canInitializeUsage:true` 和绑定的 `windowId`。能力缺省时不得发送初始化字段，不能仅凭 reader v10 推断支持。

初始化随普通 `ShareSettingsPatch.userValueUsageInitializations` 保存，形如 `{"new@example.com":{"targetPicos":"123456789123","windowId":"Day:…:…","reason":"Verified opening usage"}}`。金额为规范非负 picoUSD 整数字符串（此例为 $0.123456789123，最高 $1,000,000,000）；UI 留空或零不发送补录。非零金额须审计原因（最多 1000 UTF-8 字节），批次最多 500 个用户。必须同时提交对应新用户的 USD policy；不能与同用户的 Token `userUsageEdits` 混用。已有、已撤销、市场托管或曾有请求／保留汇总／金额历史的用户不能初始化；待下一周期生效时不能提前初始化，切换单位也不清零历史。

Router 仅在包含初始化时，为 `editContext` 加入可信 `actor`（实际登录用户，忽略浏览器提供值）；普通旧补丁不发送该新增键。Server 本地管理补丁使用本地 Owner 身份，原生 `save_provider_bundle_share` 使用已认证操作人／本地 Owner，不能由表单伪造。原生命令携带同名初始化 map 与稳定的 `valueUsageOperationId`，已有 Share 的 config CAS 规则不变。新授权、初始金额偏移、审计、水位、整份 Share 与幂等回执在同一个 SQLite IMMEDIATE 事务提交；外层取消不补录，任一步失败全部回滚这些内容，提交后响应丢失可原样重试且不重复补录。相同操作 ID 的不同 payload 拒绝；窗口变化必须重新预览。能力／绑定准备不属于金额提交，失败时可能保留安全 coverage 证据，但不能留下部分授权、金额或保存回执。

编辑弹窗区分“当前周期已消耗额度”和新用户可选初始金额：已有用户读取权威状态，未知或待生效不伪造零；可展开独立“立即校准”，明确不随外层 Share 保存／取消回滚。金额输入保留完整 pico 精度；只展示的格式化金额不得作为编辑值回填。初始化不从旧 Token 用量反推、不修改钱包、租金或市场账本。

Server 金额检查点和幂等身份不依赖 Router 30 天请求日志。托管恢复在覆盖数据库前写独立 restore fence；旧窗口证据不全则拒绝金额模式，不能删安全标记或通过切换单位重置真实用量。新旧二进制降级前必须撤销／切换并确认所有金额策略与在途数据；不允许旧版本忽略字段变成不限额。
