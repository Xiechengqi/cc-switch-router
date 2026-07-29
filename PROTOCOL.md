# Router ↔ cc-switch-server 协议契约

本文档记录 `cc-switch-router`(以下简称 Router)与 `cc-switch-server`(以下简称 Server)之间的接口契约。

Server 是 Router 的**唯一客户端**。早期的 `cc-switch` Tauri 桌面版已不再作为客户端,相关兼容代码已全部移除,详见 [MIGRATION.md](MIGRATION.md)。

本文所有断言均标注 Router 侧源码位置(`file:line`),便于与实现对账。

---

## 1. 协议常量

| 常量 | 值 | 出处 |
|---|---|---|
| `PROTOCOL_EPOCH` | `namespace-flat-1` | `src/namespace.rs:1` |
| 注册 proof 版本 | `2`(仅接受此值) | `src/store.rs:22077-22080` |
| Ingress 签名域 | `cc-switch-router-ingress-v1` | `src/ingress_context.rs:11` |

Epoch 参与所有 Ed25519 签名的规范串。两侧 epoch 不一致时**硬失败**,不做协商降级。

---

## 2. 注册:Ed25519 设备身份

`POST /v1/installations/register`

Server 首次启动时生成 Ed25519 密钥对,公钥随注册请求上送。请求体字段见 `RegisterInstallationRequest`(`src/models.rs:118-130`):
`protocolEpoch`、`publicKey`、`platform`、`appVersion`、`instanceNonce`、`timestampMs`、`signature`、`proofVersion`。

**签名规范串**(proof_version = 2,`src/store.rs:28772`):

```
{PROTOCOL_EPOCH}\nregister_installation_v2\n{public_key}\n{platform}\n{app_version}\n{instance_nonce}\n{timestamp_ms}
```

公钥与签名均为标准 base64。注册时 `installation_id` 尚不存在,故不入签名串;后续所有签名请求改用第 3 节的通用规范串。

**响应**(`RegisterInstallationResponse`,`src/models.rs:134-142`):

| 字段 | 说明 |
|---|---|
| `installationId` | Router 分配的实例 ID,后续所有请求的身份标识 |
| `controlSecret` | **对称** HMAC 密钥,与 Ed25519 密钥对相互独立。Server 必须持久化,并用它校验 Router 发来的控制平面调用与 ingress 身份头 |

> `control_secret` 是 Router → Server 方向的认证凭据;Ed25519 私钥是 Server → Router 方向的认证凭据。两者用途不可混用。

### 准入限流

注册受四层限流约束,详见 [ARCHITECTURE.md](ARCHITECTURE.md) 第 4 节。触发时返回 `429` 并携带 `Retry-After`。使用**已有公钥**恢复既有 installation 仍受尝试速率保护,但不消耗新身份额度。

---

## 3. 通用签名请求

注册之后的所有 Server → Router 签名请求,统一使用以下规范串(`src/store.rs:22565-22568`):

```
{PROTOCOL_EPOCH}\n{installation_id}\n{action}\n{payload_json}\n{timestamp_ms}\n{nonce}
```

- `payload_json` 是该请求业务载荷的 JSON 序列化结果,字段顺序必须与结构体声明一致
- `action` 为动作名,例如 `installation_setup_completed_v1`
- 签名为 Ed25519 签名的标准 base64
- `nonce` 由 Router 侧 `request_nonces` 表做重放拦截

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
4. edit-ack(`POST /v1/shares/edit-ack`)成功后,Router 将订阅从 `grant_pending` 推进到 `active_free` 或 `trial_payment_due`,或完成 revoke 后释放座位。

浏览器侧 HTTP 契约(用户 Session):

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/v1/share-market/listings` | 公开 catalog(含 `canRent`、owner blocks) |
| `POST` | `/v1/share-market/listings` | 添加 Share 挂牌(1–20 座位) |
| `DELETE` | `/v1/share-market/listings/:id` | 停止挂售 |
| `GET` | `/v1/share-market/owned-shares` | 「添加 Share」候选(`alreadyListed`) |
| `POST` | `/v1/share-market/listings/:id/seats` | 添加拼车位(可 reopen closed listing) |
| `PATCH`/`DELETE` | `/v1/share-market/seats/:id` | 编辑/删除空闲座位 |
| `POST` | `/v1/share-market/seats/:id/rent` | 租用 |
| `POST` | `/v1/share-market/subscriptions/:id/declare-paid` | 声明已付款 |
| `POST` | `/v1/share-market/subscriptions/:id/release` | 租客归还 |
| `POST` | `/v1/share-market/subscriptions/:id/force-revoke` | Owner 强制回收(可选拉黑) |
| `GET`/`DELETE` | `/v1/share-market/blocks`… | Owner 拉黑列表 / 解除 |

`alreadyListed` 为 true 当且仅当:当前 owner 对该 Share 有 `active` listing,或该 Share 上仍有非终态订阅。停止挂售且租约全部结束后可再次 `POST /listings`。

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

**该命名空间对公网封闭**:`/_share-router/*` 的入站 GET 必须携带合法控制签名,否则返回 404(`src/proxy.rs:4032-4043`)。`/_ctl/*` 在所有公网入口点一律 404,不经路由(`src/proxy.rs:1459, 1845, 2165`)。

---

## 9. 身份注入:IngressContext

Router 转发到 Server 的每个请求都会剥离客户端可伪造的凭据头(`authorization`、`x-api-key`、`cookie`,见 `src/proxy.rs:2580-2596`),改为注入一个签名的身份上下文。

结构见 `src/ingress_context.rs`:`protocolEpoch`、`routerId`、`routeId`、`installationId`、`targetLaneId`、`publicHost`、`shareId`、`requestId`、`userEmail`、`userRole`(`owner` / `admin`)、`userCountry`、`issuedAtMs`。

签名以 `control_secret` 为密钥,签名域 `cc-switch-router-ingress-v1`(`src/ingress_context.rs:11, 50`)。

Server 侧必须:

1. 校验签名
2. **校验 `issuedAtMs` 新鲜度**(Server 当前实现为 30 秒窗口)
3. 剥离客户端自带的所有 `x-cc-switch-*` 头,再由已验证的上下文重新注入

> Router 侧 `sign` 只校验 `issued_at_ms > 0`(`src/ingress_context.rs:72`),**不做接收侧重放窗口检查**。新鲜度判定完全依赖 Server 实现;若 Server 放宽该窗口,历史签名上下文可被重放。

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

## 11. 兼容策略

- **Epoch 不匹配硬失败**,不降级
- Share descriptor 同步:Server 优先调用 `POST /v1/shares/descriptor-batch-sync`;收到 404 时回落至 `POST /v1/shares/batch-sync`,并剥离 `descriptorGeneration` / `descriptorFingerprint` 字段
- 注册仅接受 `proofVersion = 2`,其余值返回错误(`src/store.rs:22077-22080`)
