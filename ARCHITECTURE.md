# portr-rs 重构分析与接入控制设计

## 1. 当前实现现状

### cc-switch 侧

- `cc-switch` 已经有 Rust 实现的 tunnel client，入口在 [src-tauri/src/tunnel/mod.rs](/data/projects/cc-switch/src-tauri/src/tunnel/mod.rs)。
- 当前启动流程是：
  1. 通过 HTTP 调 `POST /api/v1/connections/` 预留 connection。
  2. 服务端返回 `connection_id`。
  3. 客户端用 SSH 用户名 `"{connection_id}:{secret_key}"` 登录，再申请 `tcpip_forward`。
- 当前持久化配置在 [src-tauri/src/tunnel/config.rs](/data/projects/cc-switch/src-tauri/src/tunnel/config.rs)：
  - `server_url`
  - `ssh_url`
  - `tunnel_url`
  - `secret_key`

### portr server 侧

- 连接创建 API 在 [internal/server/admin/api/connection/handlers.go](/data/projects/portr/internal/server/admin/api/connection/handlers.go)。
- SSH 认证在 [internal/server/ssh/sshd.go](/data/projects/portr/internal/server/ssh/sshd.go)。
- 当前认证核心逻辑：
  - HTTP 层通过 `secret_key` 查询 `team_users`。
  - SSH 层再次解析 `connection_id:secret_key`，校验 `reservedConnection.CreatedBy.SecretKey == secretKey`。
- 这本质上是“长效静态共享密钥 + 连接 ID”的方案。

## 2. 现有方案的问题

如果 `cc-switch` 直接内嵌：

- `server_url`
- `ssh_url`
- `tunnel_url`
- `secret_key`

那么任何人只要拿到二进制、配置文件或运行时内存，就可以绕过 `cc-switch`，直接写一个脚本调用 API + SSH 接入公共 tunnel 服务。

结论：

- “把静态 secret 内嵌到客户端”不能实现“只有 cc-switch 才能穿透”。
- 它最多只能实现“默认免配置”。
- 如果目标是“强约束只有受控客户端可接入”，服务端必须改成“短期票据 + 设备身份 + 单次连接授权”，不能再接受长效共享 secret。

## 3. 关于“只有 cc-switch 可以接入”的边界

这个目标要先分清强度：

### 可实现的强目标

- 只有拿到服务端签发的短期 tunnel lease 的客户端，才能建立 tunnel。
- lease 必须绑定某个已注册安装实例、某个 share、某个 subdomain、某个过期时间。
- SSH 连接必须使用一次性凭证或短期 SSH certificate，而不是静态 secret。

### 无法靠纯客户端绝对实现的目标

- “从密码学上证明请求一定来自官方 cc-switch 二进制，且无法被逆向复制”。

原因：

- 桌面客户端的内嵌密钥最终会落到用户设备上。
- 没有硬件远程度量/平台可信执行环境时，任何本地秘密都可能被提取。

所以应把目标定义为：

- 不向客户端下发可长期复用的总密钥。
- 所有接入都必须通过服务端在线签发的短期授权。
- 即使有人提取到历史请求，也不能长期复用。

## 4. portr-rs 的建议目标

`portr-rs` 不要照搬 Go 版的 team/user/admin 全量系统，而应该先做一个面向 `cc-switch` 的“受控公共 tunnel service”。

建议第一阶段只保留四类能力：

1. Tunnel control API
2. SSH reverse forwarding server
3. HTTP/TCP proxy router
4. 最小化的状态存储与审计

Go 版里的这些能力不建议首批迁移：

- GitHub OAuth
- 多团队后台管理 UI
- 通用用户系统
- 通用 secret key 分发

因为 `cc-switch` 已经是主产品，`portr-rs` 更适合作为它的受控基础设施，而不是通用 SaaS。

## 5. 建议的 Rust 目录结构

建议在 `/data/projects/portr-rs` 做成 workspace：

```text
portr-rs/
  Cargo.toml
  crates/
    portr-rs-types/
    portr-rs-auth/
    portr-rs-store/
    portr-rs-api/
    portr-rs-ssh/
    portr-rs-proxy/
    portr-rs-server/
```

职责建议：

- `portr-rs-types`
  - 公共 DTO、枚举、错误码、配置结构
- `portr-rs-auth`
  - 安装注册、challenge、JWT/lease、SSH cert 签发
- `portr-rs-store`
  - SQLite/Postgres 抽象
- `portr-rs-api`
  - `axum`/`hyper` API
- `portr-rs-ssh`
  - 基于 `russh` 的 reverse forwarding server
- `portr-rs-proxy`
  - HTTP/WebSocket/TCP 路由
- `portr-rs-server`
  - 主程序、配置、指标、健康检查、任务调度

## 6. 认证与授权的建议方案

### 6.1 设计原则

- 客户端内嵌的只能是：
  - 服务地址
  - TLS pin/public key
  - 非敏感 app 标识
- 客户端不能内嵌：
  - 长期共享 secret
  - 可直接建立 tunnel 的固定 SSH 密码
  - 可无限注册连接的 API key

### 6.2 推荐链路

#### 第一步：安装实例注册

cc-switch 首次启动 tunnel 功能时：

1. 本地生成设备密钥对 `device_ed25519`。
2. 私钥保存到系统安全存储：
   - macOS Keychain
   - Windows Credential Manager / DPAPI
   - Linux Secret Service，降级到本地加密文件
3. 调用 `POST /v1/installations/register`，上传：
   - `device_public_key`
   - `app_version`
   - `platform`
   - `instance_nonce`
4. 服务端返回：
   - `installation_id`
   - `installation_token` 或 registration receipt

说明：

- 这里的 `installation_token` 也不能是长期万能凭证。
- 它更适合作为“已注册实例标识”，真正开 tunnel 时仍要换短期 lease。

#### 第二步：申请短期 tunnel lease

每次启动 share tunnel：

1. cc-switch 本地构造 challenge 请求。
2. 用 `device_private_key` 对 challenge 签名。
3. 请求 `POST /v1/tunnels/lease`，带上：
   - `installation_id`
   - `share_id`
   - `requested_subdomain`
   - `local_target = 127.0.0.1:15721`
   - `timestamp`
   - `signature`
4. 服务端验证设备签名、频率、share 状态后，返回：
   - `lease_id`
   - `connection_id`
   - `ssh_username`
   - `ssh_password`（一次性，60 秒）
   - 或更优：`ssh_certificate`
   - `expires_at`

#### 第三步：SSH 建链

SSH server 不再接受 `connection_id:secret_key`。

改为只接受以下任一方式：

1. 一次性用户名密码
2. 短期 SSH certificate
3. SSH public key + 服务端 challenge 验证

推荐优先级：

1. 短期 SSH certificate
2. 一次性密码

因为这最贴近现有 reverse forwarding 流程，且服务端权限边界清晰。

#### 第四步：Forward 权限校验

在 `tcpip_forward` 时继续校验：

- lease 是否未过期
- connection 是否属于该 installation
- subdomain 是否与 lease 一致
- lease 是否尚未被使用或是否允许重连

即使 SSH 登录成功，也不能跳过 lease 绑定。

### 6.3 为什么这样可以满足“只有 cc-switch 才能接”

它不能做到理论绝对防复制，但可以做到：

- 没有服务端签发的 lease，就无法建 tunnel。
- 拿到一个历史 lease，也很快失效。
- 拿到某次 SSH 一次性密码，也只能在极短窗口复用。
- 客户端不再持有可长期滥用的总密钥。

这是桌面分发模式下最实际的强约束。

## 7. 对 cc-switch 的配套改造建议

### 7.1 TunnelConfig 需要改

当前 [src-tauri/src/tunnel/config.rs](/data/projects/cc-switch/src-tauri/src/tunnel/config.rs) 里的 `secret_key` 应删除，改成：

```rust
pub struct TunnelConfig {
    pub api_base: String,
    pub ssh_addr: String,
    pub tunnel_domain: String,
    pub bootstrap_id: String,
    pub bootstrap_public_key: String,
    pub pinned_cert_sha256: Option<String>,
    pub use_localhost: bool,
}
```

说明：

- `bootstrap_id` 只是客户端识别用途，不授予 tunnel 权限。
- 真正的 tunnel 权限来自 `/v1/tunnels/lease`。

### 7.2 client 流程需要改

[src-tauri/src/tunnel/connection.rs](/data/projects/cc-switch/src-tauri/src/tunnel/connection.rs) 不能再直接 `POST /api/v1/connections/` 带 `secret_key`。

建议改成：

1. `ensure_installation()`
2. `request_tunnel_lease()`
3. `connect_ssh_with_lease()`

[src-tauri/src/tunnel/ssh.rs](/data/projects/cc-switch/src-tauri/src/tunnel/ssh.rs) 也不能再把用户名拼成 `connection_id:secret_key`。

### 7.3 设置项需要改

[src-tauri/src/commands/share.rs](/data/projects/cc-switch/src-tauri/src/commands/share.rs) 当前会把 tunnel config 持久化到 settings。

如果公共服务配置是内嵌的，建议：

- endpoint、pin、public metadata 走内嵌只读配置
- installation 私钥走系统安全存储
- lease 不落盘，只保存在内存

这样比把 `secret_key` 写进 settings 安全得多。

## 8. portr-rs 的服务端数据模型建议

建议新建而不是沿用 Go 版表结构。

### 建议核心表

#### `installations`

- `id`
- `device_public_key`
- `platform`
- `app_version`
- `status`
- `created_at`
- `last_seen_at`

#### `shares`

- `id`
- `installation_id`
- `subdomain`
- `local_target`
- `status`
- `created_at`

如果 share 仍由 cc-switch 本地定义，也可以不在 server 永久保存完整 share，只保留 tunnel session。

#### `tunnel_leases`

- `id`
- `installation_id`
- `connection_id`
- `subdomain`
- `auth_method`
- `credential_hash`
- `issued_at`
- `expires_at`
- `used_at`
- `revoked_at`

#### `connections`

- `id`
- `lease_id`
- `status`
- `remote_port`
- `created_at`
- `started_at`
- `closed_at`

#### `backends`

- `connection_id`
- `subdomain`
- `backend_addr`
- `status`
- `last_heartbeat_at`

## 9. 服务间协议建议

### API

- `POST /v1/installations/register`
- `POST /v1/installations/refresh`
- `POST /v1/tunnels/lease`
- `POST /v1/tunnels/{lease_id}/heartbeat`
- `POST /v1/tunnels/{lease_id}/close`
- `GET /v1/healthz`

### SSH

- 只开放 reverse forward 必需能力
- 禁止 shell、exec、sftp
- 禁止任意端口范围外的转发
- `tcpip_forward` 必须和 lease 绑定

### Proxy

- HTTP Host 提取 subdomain
- WebSocket 直接透传
- 后端失活自动剔除
- 对未注册 subdomain 返回稳定错误页/错误码

## 10. Rust 技术选型建议

- HTTP API: `axum`
- SSH server: `russh`
- HTTP reverse proxy: `hyper` + `hyper-util`，或直接 `axum` + 自定义转发
- DB: `sqlx`
- 配置: `serde` + `figment`/`config`
- JWT/JWS: `jsonwebtoken` 或 `josekit`
- tracing: `tracing` + `tracing-subscriber`
- metrics: `metrics` + Prometheus exporter

原因：

- `cc-switch` 客户端已经在用 Rust，协议和错误模型更容易统一。
- `russh` 可以同时复用客户端与服务端生态。
- `sqlx` 的 schema 控制更适合把安全边界显式化。

## 11. 迁移策略

### Phase 1

- 在 `/data/projects/portr-rs` 先实现最小版：
  - installation register
  - tunnel lease
  - SSH reverse forwarding
  - HTTP proxy
- 不做 admin UI
- 不做 team/user 系统

### Phase 2

- 改 `cc-switch` 接入 `portr-rs`
- 删除 `secret_key` 逻辑
- 切到 lease-based SSH auth

### Phase 3

- 加审计、限流、封禁
- 增加 metrics、日志、运营工具
- 如确有需要，再补一个内部管理面板

## 12. 最关键的结论

1. `portr` Go server 当前的安全边界依赖静态 `secret_key`，这不适合内嵌到 `cc-switch`。
2. 如果 `cc-switch` 直接内嵌现有 `secret_key`，任何人都能绕过客户端直连公共 tunnel 服务。
3. `portr-rs` 必须把认证模型改成“安装实例身份 + 短期 lease + SSH 一次性凭证/证书”。
4. 对桌面客户端而言，不能承诺理论上的“只有官方二进制能接入”，但可以实现“没有服务端在线签发短期授权就无法接入，且历史凭证无法长期复用”。
5. 这套方案才适合 `cc-switch` 内嵌公共服务配置、免用户手工配置的产品形态。
