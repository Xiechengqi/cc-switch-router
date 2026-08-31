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

它们只在 frozen baseline、migration 19/21、显式 `410` 和负向退休测试中出现；migration 21 完成后不存在可查询的旧 archive。Share Market、Client Market、`/v1/market-access/*` 与 `/v1/market-billing/*` 是 Router 内建能力，名称中的 market 不表示旧 Token Market。

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

- 当前写出版本是 **Share Contract v6**。Router 为滚动升级仍可读取 v2..v6。v5 引入本节定义的 `modelProbe` 和 Grok 媒体权限；v6 只增加 scoped quota metadata，不改变探针或市场 App-scope 语义。连接测试与半小时模型健康检查要求当前写出版本；Share Market 发布的最低版本是 `MIN_SHARE_MARKET_CONTRACT_VERSION`（v5），以便 Server/Router 滚动升级时继续接受已同步的 v5 记录。
- `capacityPoolId` 是非空匿名标识。同一 Router 下复用相同物理账号或 API key 的不同 Share URL 使用同一值,用于容量与故障域去重；该值在凭据源不变期间稳定，账号绑定或 API key 改变时必须重新派生并同步。
- `bindings` 必须包含 1 到 3 个不同 app 的 `{ app: providerId }` 绑定,app 仅允许 `claude`、`codex`、`gemini`;顶层 `appType` / `providerId` 必须对应其中一个绑定。
- `support` 表示当前对外开启的 App API。关闭某个 API 不会删除对应 binding；至少保留一个已绑定 app 为开启。未开启的 app 不接受直连、Share Market 或 Gateway 请求。
- `appRuntimes`、`appProviders` 与 `appAvailability` 只可声明已绑定 app。访问策略由 Share 级 `freeAccess` 与 `userGrants` 统一定义；v4 延续 v2 的边界,不包含分 app ACL、`appSettings` 或普通 Share 价格字段。
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

Router **从不改写** Server 返回的 descriptor,只做校验(`store::apply_share_edit_directly`);若客户端只部分应用了补丁,Router 拒绝落库而非静默持久化。

### 7.1 Share Market managed grants

内建 Share Market(`src/share_market.rs`,`/v1/share-market/*`)在租用/回收时不直接改 Server 本地文件,而是经 pending Share edit 下发 **managed grant** 操作:

| 字段 | 含义 |
|---|---|
| `patch.managedGrant.action` | `upsert` 授予拼车位 / `revoke` 回收 |
| `patch.managedGrant.entitlementId` | 稳定 entitlement ID,与订阅绑定 |
| `patch.managedGrant.policy` | 拼车位并行/Token 限制、周期及显式 `allowedApps`(upsert 必填) |
| Server 落库 `userGrants[].manager` | 固定为 `routerShareMarket` |

Server 要求:

1. 普通 `share/settings` 入口拒绝带 `managedGrant` 的补丁。
2. pending-edit 应用路径接受 managed grant,写入/移除 `routerShareMarket` grant。
3. 普通用户编辑不得修改或删除 **仍为 active** 且 `manager=routerShareMarket` 的 grant。撤销后的 tombstone（`active=false`）保留作历史/用量 deprecated 标记，不再占用该邮箱；普通编辑可将同一邮箱写成新的 Manual `shareto`。不得把 tombstone 重新点亮为市场授权；再次出租仍只走 `managedGrant.upsert`。
4. edit-ack(`POST /v1/shares/edit-ack`)成功后,Router 将订阅从 `grant_pending` 推进到 `active_free` 或 `active_postpaid`,或完成 revoke 后释放座位。

一个拼车位授权整个 Share,不出售单一 App。报价会冻结当时全部已启用 App 的供应商与模型条款(`service.apps[]`),租客不能再通过 `requiredApp` 收窄授权；该旧请求字段仅作为兼容输入保留并被忽略。Router-managed grant 对三个核心 App 显式下发 `allowedApps=[claude,codex,gemini]`,Server 再以目标 App 当前是否已绑定且启用作为实际可调用边界，因此 Share 后续启用的核心 App 无需重建租约即可使用。升级前的单 App 活跃 grant 通过幂等 upsert 原地扩权,不 revoke、不暂停服务或账单；迁移时已经在途的 v1 初次授权或恢复授权可完成一次旧范围确认，但不会标记为扩权完成，下一轮立即原地扩权。v2 租约不接受该兼容行为。

用户“已消耗 Token / 周期起点”重基线只由 Server Provider 编辑入口提交 `userUsageEdits`；Router Share Market grant 仍只读。重基线变化会带来新的 descriptor generation，普通请求 usage 计数不会触发同步风暴。

浏览器侧 HTTP 契约(用户 Session):

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/v1/share-market/listings` | 公开 catalog(含统一准入与授信计算后的 `canRent`) |
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

`alreadyListed` 作为兼容字段,在当前 owner 有 active listing 或 Share 仍有非终态订阅时为 true；新 UI 以 `canCreateListing`、`activeListingId` 与 `reopenListingId` 决定动作。只要存在未删除的 closed listing,`POST /listings` 返回 `share_market_reopen_required` 及原 `listingId`,不得创建重复 listing。

恢复请求至少包含一个 `existingSeats[]` 或 `newSeats[]`。旧车位项提交 `seatId`、当前 `offerRevision` 和完整 `seat` 条款；只有 `disabled`、未退休、无当前订阅且属于该 listing 的车位可重新发布。Router 使用 Immediate 事务校验 owner、Share 状态/归属、公开免费策略、待应用编辑、Client contract 版本、Token 周期、Share 并发上限、付费资料、20 车位上限及其他 active listing,再统一更新旧车位、插入新车位并激活原 listing。任何失败都会完整回滚。进行中的租约既不阻止恢复,也不会被恢复操作修改。

停止挂售是幂等且可逆的。首次停止将 `available` 车位改为 `disabled` 并递增 `offerRevision`,同时把该 listing 下所有 active rent quote 改为 `expired`；重复停止不会再次递增 revision 或重复写审计事件。恢复旧车位时再次递增 revision,因此停止前生成的 quote 即使仍在客户端内存中也不能在恢复后提交。

免费和付费拼车位都可通过 `serviceDurationDays` 设置 `1..=365` 天固定服务期限；省略或传 `null` 表示无固定期限。`tokenLimit` 省略时 Router 忽略提交的 `tokenPeriod` 并归一为 `lifetime`；只有设置 Token 限额时才校验该 Server 支持的周期。报价参数在租用时冻结到 Subscription，固定期限从租用事务提交成功时计算，并以同一绝对 `expiresAt` 写入 managed grant policy，授权延迟和账单暂停均不顺延。Router 在到期前 24 小时写入一次 `service_term_expiring`，到期后终止付费合约并根据控制操作是否可能已送达选择直接释放或安全 revoke；付费尾段会精确结算到 `expiresAt`，账务与 Share worker 的先后顺序既不会漏计也不会越界计费。`grant_pending`、活跃、账单暂停、恢复中及控制失败状态均不得在到期后重新获得访问，回收失败时也不会把座位提前恢复为可租。

### 7.2 Share / Client Market 统一准入与授信

Share 与 Client Host 的新租用都执行供应商准入，但策略按「产品 + 价格类型」拆成四个独立作用域：`share/free`、`share/paid`、`client_host/free`、`client_host/paid`。免费作用域隐式默认 `blacklist`，便于未知用户先体验；付费作用域隐式默认 `whitelist`，必须先建立信任和授信。供应商可按规范化邮箱添加买家，买家尚未注册时可预授权；首次租用时 Router 按已验证邮箱绑定 `buyer_user_id`。每个关系可对四个作用域分别设置 `inherit`、`allow` 或 `deny`。

- 白名单模式下，只有有效关系且对应作用域明确 `allow` 的买家可新租；黑名单模式下，除对应作用域明确 `deny` 外均可新租。免费或付费、Share 或 Host 的规则互不隐式继承。
- 付费租用还要求同一买家和供应商存在 USD `limited` 或 `unlimited` 私有授信。有限额度是账户自动出账边界；无限额度不自动出账,由任一方发起清账。
- 只有从白名单实际切换到黑名单时必须提交风险确认；重复保存黑名单不重复要求确认。供应商可另行开启有限公共额度供付费黑名单作用域中的未知买家使用，但公共额度不能设为无限。
- `GET /v1/share-market/listings` 的座位与 `GET /v1/client-market/hosts` 的 Host 都返回 `sellerApprovalRequired`。该字段只面向已登录的非 Owner,表示当前供应商准入不允许该买家；前端据此保留「租用」/「新建」入口并引导联系 Owner,不得把服务端英文拒绝消息直接展示为红色错误。Share 引导到对应 Client 聊天室；Client Host 展示 Owner 邮箱及其公开联系方式。
- 模式切换和产品规则更新只影响新租用。撤销整个买家关系会把该买家的账户信用设为 `none` 并终止现有付费服务；以后确认历史账单也不会恢复这些服务。现有免费服务不因单独修改策略而中断,Owner 可另行强制回收。
- 所有更新操作使用 revision 做乐观并发控制；下列 `PUT` 请求必须提交当前资源的 `expectedRevision`，新资源提交 `0`。浏览器可用用户 Session；外部系统可用用户 API Token,读取和写入分别要求 `market:access:read`、`market:access:write` scope。
- 白名单买家可从具体 Share seat 或 Client Host 发起「申请准入」。同一买家、供应商、产品和价格作用域只保留一个待处理申请；拒绝后 24 小时内不能重复申请。申请记录目标名称、当时的日费和币种供供应商判断，供应商的「市场准备」与「市场准入」入口显示待处理数量；最终租用仍校验商品当前 `offerRevision`。
- 免费申请批准只允许对应免费作用域。付费申请必须在同一 libSQL Immediate 事务中同时允许对应付费作用域并授予有效 USD 有限或无限额度；任一步失败时关系、规则、额度和申请状态全部回滚。供应商拒绝必须提交原因，买家可用申请 revision 取消仍在等待的申请。

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/v1/market-access/dashboard` | 读取产品模式、可信买家、授信与当前风险敞口 |
| `GET` | `/v1/market-access/inbox-summary` | 读取供应商待处理准入申请数量，用于导航角标 |
| `PUT` | `/v1/market-access/policies/:product_kind/:pricing_kind` | 独立切换四个作用域的白名单或黑名单模式 |
| `POST` | `/v1/market-access/counterparties` | 按邮箱创建或重新启用可信买家关系 |
| `PUT` | `/v1/market-access/counterparties/batch` | 原子保存多个买家关系、四作用域规则和 USD 授信 |
| `PUT` | `/v1/market-access/counterparties/:id` | 更新产品规则或撤销关系 |
| `PUT` | `/v1/market-access/counterparties/:id/credit-lines/:currency` | 更新买家 USD 私有信用额度 |
| `PUT` | `/v1/market-access/public-credit-lines/:currency` | 更新黑名单模式的有限公共额度 |
| `POST` | `/v1/market-access/requests` | 买家按 `targetKind + targetId` 申请对应供应商作用域 |
| `POST` | `/v1/market-access/requests/:id/approve` | 供应商批准；付费申请可原子携带信用额度 |
| `POST` | `/v1/market-access/requests/:id/reject` | 供应商填写原因并拒绝申请 |
| `POST` | `/v1/market-access/requests/:id/cancel` | 买家取消自己的待处理申请 |

写入请求使用 camelCase JSON，未知字段会被拒绝：

| 路径 | 关键请求字段 |
|---|---|
| `PUT /policies/:product_kind/:pricing_kind` | `mode`、从白名单切换到黑名单时的 `riskAcknowledged: true`、`expectedRevision` |
| `POST /counterparties` | `email`、`accessRules[] { productKind, pricingKind, decision }`，可选初始 `creditLines[] { currency, kind, limitMinor?, riskAcknowledged? }` |
| `PUT /counterparties/batch` | `updates[] { id, expectedRevision, status?, accessRules[], creditLines[] }`；每条 `creditLines[]` 还携带自己的 `expectedRevision` |
| `PUT /counterparties/:id` | 本次变更的 `accessRules[] { productKind, pricingKind, decision }`、可选 `status`、`expectedRevision` |
| `PUT /counterparties/:id/credit-lines/:currency` | `kind`(`none` / `limited` / `unlimited`)、有限额度的 `limitMinor`、无限额度的 `riskAcknowledged: true`、`expectedRevision` |
| `PUT /public-credit-lines/:currency` | `enabled`、启用时的有限 `limitMinor` 与 `riskAcknowledged: true`、`expectedRevision` |
| `POST /requests` | `targetKind`(`share_seat` / `client_host`)、`targetId` |
| `POST /requests/:id/approve` | `expectedRevision`；付费申请在新增或修改授信时携带 `creditLine { currency: "USD", kind, limitMinor?, riskAcknowledged?, expectedRevision }`，沿用既有有效授信时可省略，免费申请不得携带 |
| `POST /requests/:id/reject` | `expectedRevision`、必填 `reason` |
| `POST /requests/:id/cancel` | `expectedRevision` |

`limitMinor` 使用 USD 最小单位（美分）且范围为 `1..=100000000`；路径币种仅接受 `USD`。私有无限额度和任何公共额度都必须显式确认风险，公共额度始终只能是有限额度。

Client Host 继续使用独立的免费期限契约：Host 创建、编辑与导入接口接受 `freeDurationDays=1..365` 或 `null`（永久），付费 Host 拒绝该字段。Allocation Quote 冻结期限和 `offerRevision`；倒计时从 Client provisioning 成功、订阅写入 `activatedAt` 时开始。到期前 24 小时只产生一次临期事件，到期后 Router 以 `free_period_expired` 调用现有安全 cleanup。清理失败时租约保持 `release_failed` 且 Host 继续隔离，不会错误回到 `idle`。

### 7.3 Share / Client Market 统一后付费

付费 Share 与 Client Host 不再按单个商品预付或续费。Router 按「买家 + 供应商」维护唯一 USD 赊账账户；每个服务独立享受 12 小时健康时长试用,之后按固定 USD 每日价格和实际健康秒数累计。有限额度使用达到 80% 时向买卖双方各发送一次预警,用满后自动生成聚合账单。买家主动清账、供应商要求清账、供应商永久关闭赊账账户或最后一个服务结束时也会生成聚合账单。

USD 是唯一报价、授信、记账和结算币种。账单总额与每条账单明细同时返回 `amountUsdMinor` 和 `amountCnyMinor`，其中 `amountMinor` 继续作为 USD 兼容字段。美元兑人民币汇率由 `CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE` 控制，默认 `1:7`，可在 Settings 热更新；未出账估算使用当前汇率，账单通过 `usdCnyRateMicros`（百万分之一精度）冻结出账汇率及人民币金额，后续设置变更不影响历史账单。人民币金额四舍五入到分且仅用于展示。`/v1/market-billing/supplier-profiles/:currency` 的路径参数只接受 `USD`。

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/v1/market-billing/config` | 当前 USD 记账币种及美元兑人民币展示汇率 |
| `GET` | `/v1/market-billing/dashboard` | 买方应付、供应商应收、当前账单与赊账限制 |
| `PUT` | `/v1/market-billing/supplier-profiles/:currency` | 设置账单付款宽限时间 |
| `POST` | `/v1/market-billing/accounts/:id/settle` | 买方主动生成当前聚合账单 |
| `POST` | `/v1/market-billing/accounts/:id/request-settlement` | 供应商要求买家结清当前余额,包括无限额度账户 |
| `POST` | `/v1/market-billing/accounts/:id/close` | 供应商永久终止关系；无账单时生成最终账单，已有账单时锁定关闭意图 |
| `GET` | `/v1/market-billing/accounts/:id/invoices` | 按 sequence 游标分页读取已结账单 |
| `POST` | `/v1/market-billing/invoices/:id/declare-payment` | 买方提交链下付款声明 |
| `POST` | `/v1/market-billing/invoices/:id/confirm` | 供应商核验并确认到账 |
| `POST` | `/v1/market-billing/invoices/:id/reject` | 供应商拒绝付款声明 |
| `POST` | `/v1/market-billing/invoices/:id/disputes` | 买方发起一次账单争议 |
| `GET`/`POST` | `/v1/admin/market-billing/disputes…` | 管理员查看并裁决争议 |
| `POST` | `/v1/admin/market-billing/invoices/:id/void` | 管理员强制作废账单 |

每张账单在生成时冻结供应商当时的收款方式、联系方式和资料更新时间；后续修改供应商收款资料不会改写历史或未结账单。付款声明和发起争议都不代表债务已解除。账单到达截止时间后,即使处于 `payment_declared` 或 `disputed` 状态也会保留全局市场赊账限制；只有供应商确认到账或管理员作废账单才解除该账单对应的限制。服务暂停期间不计费。

供应商可在账户已有待付、待确认、逾期或争议账单时继续执行永久关闭。关闭意图一经锁定即终止全部关联服务；后续确认到账或管理员作废账单只结清债务,不会恢复服务,双方也不能再次建立付费租约。

### 7.4 Market 系统事件与 Client 公开聊天室

Share Market、Client Market 和统一账务不创建商品级聊天室。Router 只按 `installation.id` 建立唯一 Client 公开房间；Share Market 表格和 Client Market Provider/租客入口均解析到对应 `GET /v1/chat/clients/:installation_id/room`。同一 Client 的多个 Share 必须返回同一个 room ID。

关键业务事件与原业务写入处于同一 libSQL 事务,先进入 `client_chat_system_outbox`,再由后台幂等物化为系统消息。Share 服务启停、到期、供应商变更、离线/恢复；挂牌、拼车位、租用、授权、回收；Client 开通、清理开始/成功/失败、强制释放；额度预警、出账、付款声明/确认/拒绝、逾期、争议和裁决均属于该事件流。系统消息自动关注 Owner、Provider、租客和 actor,产生未读,但不触发真人聊天提醒邮件。

事件 payload 是公开数据。Router 原样公开完整邮箱、账单金额、收款方式/联系方式、付款 reference/note、凭证 URL、争议/回收原因和安全的原始错误。系统消息中由 Owner/Provider/Supplier 发布的 `/v1/account/payment-assets/:id` 收款图片也允许匿名读取；未被系统消息引用的资产继续要求 Owner 或对应账单买方 Session。以下内容是唯一保密例外,不得出现在 DB、API、日志或 UI：API Key、OAuth/Session token、Cookie、Authorization、密码、secret、私钥以及 SSH/lease 凭据。`PaymentMethod.token` 仅在 `kind=crypto` 且值为 `USDT`/`USDC` 时作为公开资产符号放行,其他 `token` 字段均按凭据拒绝。后端入 outbox 前拒绝敏感字段和带凭据 query/userinfo/fragment 的 URL,错误文本命中凭据片段时整体替换为固定占位；Web UI 还会独立执行字段、文本和 URL 二次过滤。

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

已启用但没有可执行探针的高优先级 App 会被跳过并继续尝试下一 App；三者均不可测试时关闭当前资格 epoch,不会用硬编码请求补位。Router 先按 installation 分组,最多并行处理 16 个 installation；控制路由优先使用 installation 的 Client 隧道,再回退最多两个当前在线的 Share subdomain。同一 installation 内的 256 条分块保持顺序,每个已 claim 批次的全部 v2 路由尝试和可选 v1 回退共享 7 分钟总预算,且最晚不得超过槽结束后 5 分钟。v1 没有 canonical observation 幂等保证,因此每批最多回退一次；模糊传输失败记为 monitoring gap,不经另一 Share 路由重复消费模型请求。Server 在 batch 内按 Provider runtime 分组,最多并行探测 3 组,完成后按 App/Provider key 确定性排序。单个目标已删除、禁用或绑定失效时 Server 省略该结果；单个 Provider 组执行异常也只省略该组,不得让其他 Provider/Share 连带失败。Router 只把对应缺失目标记为 monitoring gap。

`cycleId` 是确定性的 `utc-{slot_start}`。Server 按 Provider 运行时去重执行,一次实际探测可以投影到多个 Share；相同 cycle 的 HTTP 重试复用 health fingerprint 完全相同且内容稳定的 Provider 结果。Server 自己的半小时 Provider scheduler 在最近 45 分钟已有 Router cycle 时跳过对应 Provider,避免双重消耗。

v2 每条结果必须携带 `observationId`、`outcome`、`failureDomain`、`reasonCode`、`evidenceScope=provider_runtime` 与 `evidenceVersion=2`。`observationId` 是 installation、cycle、App、Provider 和 health fingerprint 的稳定 SHA-256；Router 会重新计算,并严格匹配 App、Provider、`requestedModel`、`actualModel`、模型策略和 health fingerprint。`actualModel` 在固定上游策略下是固定上游模型,在透传策略下是 probe 的 wire model。一次 Provider observation 只写入 `share_model_probe_observations` 一次,各 Share 槽只保存其投影。相同 ID 的状态、时间、模型、延迟或故障分类发生漂移时整笔投影回滚。

故障域固定为:上游/网络/限流属于 `upstream`,账户额度阻断属于 `quota`,鉴权、模型名和 Provider 协议配置属于 `provider_config`,隧道与控制请求属于 `control_transport`,Router 编解码/校验/持久化属于 `router_monitor`,无法稳定分类才使用 `unknown`。前三类是已实际观察到的上游侧失败；后两类没有观察到 Provider,必须写成 `outcome=unobserved` 并计入 monitoring gap,不得伪装成上游失败。v1 回退结果标为 `share_legacy`/`evidenceVersion=1`,继续展示但不得用于供应商信誉、SLA、退款或结算。

每次监测配置的有效期由 probe epoch 精确表达,包含 App、Provider、容量池、测试模型、模型策略和 health fingerprint。启用、暂停、恢复或配置切换均开启/关闭 epoch；当前槽已经被领取时,切换从下一槽生效。完整资格日分母为 48；首次启用日、暂停日和恢复日仅计算实际落在 epoch 内且已经开始的槽。Share 运行前、暂停窗口和未来槽不进分母。

`GET /v1/shares/:share_id/model-health-calendar?days=N` 返回 UTC 日历聚合,`N` 被限制在 1..400,Dashboard 默认读取 365 天。每个 active 日同时返回 eligible、completed、observed、successful、已确认上游侧失败、monitoring gap、成功率和覆盖率；成功率固定为 `successful / eligible`,覆盖率为 `observed / eligible`,缺失检查不能算成功。历史槽、canonical observation 与已结束 epoch 保留 400 天。日历只对以下调用方可见:活跃公开 Share Market listing 的访客、Share Owner、管理员、活跃 ShareTo 用户。

Client 页和 Share Market 页的 Share 侧边栏使用同一个日历组件:日期方块按红、黄、浅绿、绿表达上游模型可用率,并显示月份横轴和星期纵轴。Tooltip 必须同时说明已观测数、确认的上游侧失败、监测缺口、覆盖率和同日配置切换；共享结果只标记“共享上游探针”,不披露其他 Share 数量。这一图表不能混用 30 秒连通率。

### 8.2 连接示例与手动测试

“连接”弹窗把手动操作明确标为**端到端连接测试**,对每个已启用 App 只显示一条 Server-authoritative 模型请求。Curl 的 method、path、body、流式 header 和测试模型全部由对应 `appRuntimes.<app>.modelProbe` 生成；Router 的手动 `test-connection` 经公开 Share 路由执行这份 probe 并按 `responseMode` 校验 JSON 或 SSE 完成事件。半小时定时探针则由 Server 在内部对绑定 Provider 执行同一测试模型。两者共用模型/策略来源,但证据范围不同,前端和 Router 后端不得保留硬编码的 GPT、Claude 或 Gemini 测试模型表。

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

无论命中哪一级，转发给 Share 的模型名**恒等于**客户端请求的模型名——Router 不改写请求体中的 `model`，由目标 Share 自行决定固定上游或继续透传。除用户显式写入的 `*` 记录外，绝不做模型改写、系统推断的默认 Share、按在线状态选 Share、前缀/后缀/正则/大小写不敏感匹配、fallback 或跨 Share retry。`*` 命中的请求与精确命中走完全相同的 Share 存在性、App 开启状态、用户 ACL 与活动路由校验，同样 fail closed。找到映射后仍在当前数据库快照中重新检查 Share 存在性、App 开启状态和用户 ACL，再确认相同 Share ID 的活动内存路由，最后把请求交给既有 `proxy_handler`。后者会再次执行 Share edge ACL、并发、请求体限制、IngressContext、流式生命周期和响应清洗。

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
