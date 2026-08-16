# Router ↔ cc-switch-server 协议契约

本文档记录 `cc-switch-router`(以下简称 Router)与 `cc-switch-server`(以下简称 Server)之间的接口契约。

Server 是唯一可以注册为 **Client installation**、建立隧道并出现在 Client 监控中的程序。Dashboard 浏览器和 Market 服务使用独立的 **auth device** 身份,不会创建 Client。

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

Dashboard、公开 Share Web 和 Market 服务生成独立 Ed25519 密钥对并注册 auth device。请求字段为:
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
- `payload_json` 是该请求业务载荷的 JSON 序列化结果,字段顺序必须与结构体声明一致
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

- `capacityPoolId` 是非空匿名标识。同一 Router 下复用相同物理账号或 API key 的不同 Share URL 使用同一值,用于容量与故障域去重；该值在凭据源不变期间稳定，账号绑定或 API key 改变时必须重新派生并同步。
- `bindings` 必须包含 1 到 3 个不同 app 的 `{ app: providerId }` 绑定,app 仅允许 `claude`、`codex`、`gemini`;顶层 `appType` / `providerId` 必须对应其中一个绑定。
- `support` 表示当前对外开启的 App API。关闭某个 API 不会删除对应 binding；至少保留一个已绑定 app 为开启。未开启的 app 不接受直连、Market 和 Gateway 请求。
- `appRuntimes`、`appProviders`、`appSettings` 和分 app 价格只可声明已绑定 app。多 app Share 的远程 ACL、限额、到期时间、描述、子域名和价格百分比必须一致。
- `upstreamProvider`、`appRuntimes` 和 `appProviders` 中的 Provider 投影携带有效 `modelPolicy`，并用 `modelPolicyScope=global|per_app` 与 `modelPolicySource=bundle_global|app_independent|profile_fixed` 明确控制来源。`global` 只统一 Bundle 中可配置的 Surface；Profile 固定策略可不同且必须标记为 `profile_fixed`。这些字段属于静态 descriptor 指纹，单独切换 scope 也必须提升投影并同步 Router。
- 调用 app 只由 URL 协议路径判定,客户端提供的 app header 不参与授权。未绑定 app 的直连、Market 和 Gateway 请求均被拒绝。

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

Router 在收到 `tcpip_forward` 后绑定本地 TCP 监听(`0.0.0.0`/`::` 归一化为 `127.0.0.1`),注册为候选路由。`market-http` 类型直接提升为活跃;client-web 与 share 隧道需经 `activate` 显式提升。

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

`market-http` 是第三种类型,由 Router 侧市场组件使用(`src/store.rs:5932`),不由 Server 申请。

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
| `patch.managedGrant.policy` | 拼车位并行/Token 限制与周期(upsert 必填) |
| Server 落库 `userGrants[].manager` | 固定为 `routerShareMarket` |

Server 要求:

1. 普通 `share/settings` 入口拒绝带 `managedGrant` 的补丁。
2. pending-edit 应用路径接受 managed grant,写入/移除 `routerShareMarket` grant。
3. 普通用户编辑不得修改或删除 `manager=routerShareMarket` 的 grant。
4. edit-ack(`POST /v1/shares/edit-ack`)成功后,Router 将订阅从 `grant_pending` 推进到 `active_free` 或 `active_postpaid`,或完成 revoke 后释放座位。

浏览器侧 HTTP 契约(用户 Session):

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/v1/share-market/listings` | 公开 catalog(含统一准入与授信计算后的 `canRent`) |
| `POST` | `/v1/share-market/listings` | 添加 Share 挂牌(1–20 座位) |
| `DELETE` | `/v1/share-market/listings/:id` | 停止挂售 |
| `GET` | `/v1/share-market/owned-shares` | 「添加 Share」候选(`alreadyListed`、`subdomain`、`ownerEmail`、`supportedApps`) |
| `POST` | `/v1/share-market/listings/:id/seats` | 添加拼车位(可 reopen closed listing) |
| `PATCH`/`DELETE` | `/v1/share-market/seats/:id` | 编辑/删除空闲座位 |
| `POST` | `/v1/share-market/seats/:id/rent` | 租用 |
| `POST` | `/v1/share-market/subscriptions/:id/release` | 租客归还 |
| `POST` | `/v1/share-market/subscriptions/:id/force-revoke` | Owner 强制回收,可同时拒绝该买家后续 Share 租用 |

`alreadyListed` 为 true 当且仅当:当前 owner 对该 Share 有 `active` listing,或该 Share 上仍有非终态订阅。停止挂售且租约全部结束后可再次 `POST /listings`。

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
| `POST /_share-router/model-health` | 对绑定的上游供应商做实时健康探测 |

校验方式与第 7 节相同(同一 HMAC 契约),另需 `x-cc-switch-share-id` 头标识目标 share。

### 8.1 Share 请求日志 usage 生命周期

Server 通过签名接口 `POST /v1/share-request-logs/batch-sync` 上送同一 `requestId` 的创建和终态记录。Token 数字字段为兼容字段，是否已经观测到真实 usage 必须由以下字段判定：

| 字段 | 语义 |
|---|---|
| `usageState` | `pending`、`observed`、`missing`、`parse_error` 或 `interrupted` |
| `streamStatus` | 上游流终态或失败原因，例如 `completed`、`client_cancelled`、`timeout` |
| `usageRevision` | 同一请求内单调递增的 revision |

- `observed` 允许所有 token 字段都为 `0`；这表示上游明确返回零，不能等同于 unknown。
- `pending`、`missing`、`parse_error` 和 `interrupted` 的数字兼容字段即使为 `0`，Dashboard、ticker、吞吐量和统计采样也不得当作真实零 usage。
- `share_request_logs` upsert 只接受 `excluded.usage_revision >= share_request_logs.usage_revision`，因此迟到或重放的低 revision pending 不能覆盖更高 revision 终态。
- 旧 Server 未发送这些字段时，Router 按 `usageState=observed`、`usageRevision=0` 兼容；新 Server 必须发送明确状态。

**该命名空间对公网封闭**:`/_share-router/*` 的入站 GET 必须携带合法控制签名,否则返回 404(`src/proxy.rs:4032-4043`)。`/_ctl/*` 在所有公网入口点一律 404,不经路由(`src/proxy.rs:1459, 1845, 2165`)。

---

## 9. 身份注入:IngressContext

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

### 9.0 请求体上限声明:`x-cc-switch-ingress-body-limit`

与签名上下文同批注入,但**不参与签名**:值是十进制字节数,等于 Router 为本次请求命中的档位上限(普通 / 视频 / 图片,见 `src/proxy.rs` `proxy_request_body_limit()`)。

Server 侧契约:生效上限取 `min(本地上限, 声明值)`。因此该头无需签名——伪造只能把上限压低(伪造者自伤),抬不高 Server 的本地配置;Router 还会通过 `is_internal_share_context_header()` 剥离来自公网的同名头,Server 看到的值只可能由 Router 写入。

两端可独立升级,任意顺序:

- 旧 Server 不认识该头 → 沿用自身硬编码上限,行为不变。
- 旧 Router 不发送该头 → 新 Server 回退到历史默认值(普通 2 MiB / 视频 32 MiB / 图片 48 MiB)。

Server 的本地上限默认取 Router 允许的最大档位,使 Router settings 成为默认的唯一天花板;Server owner 可用 `requestBodyLimits` / `CC_SWITCH_{,MEDIA_,IMAGE_}REQUEST_BODY_LIMIT_MB` 主动收紧。

### 9.1 推理身份与本地并发错误

- Direct Share 使用 Router API Token 解析出的规范化邮箱作为 `IngressContext.userEmail`。
- Market 使用已认证 Market Session 的规范化邮箱作为 `IngressContext.userEmail`。不得只在 Router 做授权后向 Server 签发空用户身份。
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

## 10. 边界策略

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

## 11. 协议严格性

- **Epoch 不匹配硬失败**,不降级
- Client installation 注册只接受 `register_installation` 规范串;auth device 注册只接受 `register_auth_device` 规范串
- 注册字段全部必填且拒绝未知字段,不存在注册 proof 版本字段、无签名注册或旧签名串分支
- Client installation 注册响应必须包含 `controlSecret`;Server 不接受缺少该字段的成功响应
- Share descriptor 同步:Server 优先调用 `POST /v1/shares/descriptor-batch-sync`;收到 404 时回落至 `POST /v1/shares/batch-sync`,并剥离 `descriptorGeneration` / `descriptorFingerprint` 字段
