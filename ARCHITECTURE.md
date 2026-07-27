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
| `share descriptor` | Server 同步给 Router 的 share 配置与运行时快照 |
| `account` | 绑定在 Upstream Provider 上的凭据(OAuth token 或 API key),存于 Server |
| `control_secret` | 注册时下发的对称 HMAC 密钥,用于 Router → Server 方向认证 |
| `ingress context` | Router 注入转发请求的签名身份上下文 |
| `pending share edit` | Router 侧排队的 share 变更,由 Server 拉取并 ack |

---

## 1. 系统定位

cc-switch-router 是 TokenSwitch 的**公共汇聚层**。它为 `cc-switch-server` 实例提供公网可达性,并在此之上承载三层市场。

单进程同时承载三个职责:

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

核心依赖:`axum`、`russh`、`rusqlite`、`tokio`、`reqwest`。

**客户端**:`cc-switch-server` 是唯一客户端。`install-client.sh` 负责在远程主机上部署它。

**多区域**:仓库根部 `regions` 文件声明区域到域名的映射,当前为 `japan` / `singapore` / `hongkong` / `usa` 四个区域,经 `GET /v1/regions` 暴露(`src/api.rs:121, 2340`)。

---

## 2. 三层市场模型

三个市场共用同一套隧道内核,分别解决供应链上不同环节的问题。

### ① Share Market —— 额度共享

卖方拥有 Claude / Codex 等订阅,在自己机器上运行 Server,通过 Router 获得公网子域名。买方使用 Router 签发的 API token 调用 `/v1/messages`、`/v1/responses`、`/v1/chat/completions`,流量经 Router 路由至卖方 Server,由卖方凭据完成真正的上游调用。

定价按**官方价百分比**、分 app 独立设置:

```rust
for_sale_official_price_percent_by_app: BTreeMap<String, u16>  // models.rs:2211
```

配套机制:

- `token_limit` / `parallel_limit` —— 每个 share 的额度与并发上限
- `share_request_logs`、`llm_request_metrics` —— 逐请求计量(模型、token 数、延迟、估算成本)
- `share_model_health_state` —— 滚动健康度,支撑 share 间自动 failover
- `free_share_ip_parallel_limit` —— 免费档按真实用户 IP 限并发

> **关键性质:Router 不持有上游凭证。** `ShareUpstreamProvider` 绑定在卖方 Server 侧,凭据始终留在卖方机器上。Router 只做路由、计量、鉴权和脱敏,不是凭证托管方。

### ② Client Market —— 主机供给

Host Provider 贡献一台 Linux 服务器,Router 用**专用外发 Ed25519 provision key**(与入站 SSH host key 相互独立,`src/provision_ssh.rs:27`)登录,安装依赖并部署 Server,使其成为市场 ① 的一个供给节点。

主机状态机(`router_ssh_hosts.status`):

```
idle ──► locked ──► allocated ──► draining ──► idle

异常态:unreachable / abnormal / disabled / reserved
```

`reserved` 用于报价锁定期(`client_market_trade.rs:29`)。

**自愈机制**(`src/client_market.rs`):

| 场景 | 处理 |
|---|---|
| `draining` 停滞超 10 分钟 | 自动派发 cleanup job |
| `unreachable` 超 5 分钟 | SSH 探测;确认无安装痕迹则清除 DB 记录并复位为 idle |
| 进程重启导致 job 中断 | 启动时 `reconcile_interrupted_jobs` 以最多 4 并发重跑 |

**支付为链下荣誉制**:租客调用 `declare-paid` 自报已付,Host Provider 线下核验。Router 存储支付方式二维码资产与声明记录,**不经手资金流**。

### ③ Router 联邦 —— 横向扩展

其他 Router 与 Gateway 以带 scope 的合作方身份接入,消费 share 挂牌数据:

| 表 | 主体 | 认证 |
|---|---|---|
| `router_markets` | 市场合作方 | bearer token |
| `router_gateways` | 网关合作方 | HMAC 签名(`x-cc-gateway-*` 头 + body SHA-256) |

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
| 新身份配额 | 10 分钟 / 小时 / 日 滑窗 | SQLite(重启不重置) |
| 未绑定 Owner 水位 | 默认 50000,达到后暂停新身份准入 | SQLite |
| 请求并发 | 6 个 `KeyedConcurrencyLimiter` | 内存(`proxy.rs:562-573`) |
| 认证滥用 | 10 分钟 10 次失败 → 封禁 1 小时 | 内存(`abuse.rs:6-8`) |

并发限流键位:`share_id`、`share_id:app`、`share_id:app:email`、用户 IP(免费档)、图片任务、市场邮箱。

> **已知限制**:内存 TokenBucket 无持久化,进程重启即清零。SQLite 侧的新身份配额能兜住真正的身份创建,但尝试洪水本身在重启瞬间不受限。

### 真实客户端 IP

`src/cf.rs` **不调用任何 Cloudflare API**,它只硬编码 Cloudflare 的 IPv4/IPv6 网段(`cf.rs:15-42`),用于判断 TCP peer 是否为 CF 边缘节点。

- peer 是 CF → 信任 `CF-Connecting-IP` / `CF-IPCountry` / `CF-ASN`
- peer 非 CF → 回退 socket peer IP

这防止伪造头绕过免费档限流(`client_meta.rs:11`、`proxy.rs:4122`)。

---

## 5. 数据层

主库与 metrics 库分离,共 78 张表。

**连接模型**:单个 `Arc<Mutex<Connection>>`,WAL 模式、外键开启、`busy_timeout = 5000ms`(`store.rs:1241-1246`)。

**迁移策略**:无版本表。`CREATE TABLE IF NOT EXISTS` 建表,再经 `PRAGMA table_info` 探测后 `ALTER TABLE ADD COLUMN` 补列(`store.rs:13068+`)。每次启动全量重跑,幂等但无回滚路径。

### 表分组

| 域 | 代表表 |
|---|---|
| 身份与隧道 | `installations`、`leases`、`tunnel_route_heads`、`shares`、`installation_client_tunnels` |
| 计量 | `share_request_logs`、`market_request_logs`、`llm_request_metrics`、`image_generation_*` |
| 健康 | `share_health_checks`、`installation_health_checks`、`share_model_health_state` |
| 主机市场 | `router_ssh_hosts`、`client_market_subscriptions`、`client_market_invoices`、`account_payment_*` |
| 联邦 | `router_markets`、`router_gateways`、`share_market_listing_statuses` |
| 通知 | `client_notification_events`、`email_delivery_batches` 等 9 张 |
| 聊天 | `client_chat_rooms`、`client_chat_messages` 等 7 张(`store/client_chat.rs`) |
| 认证 | `users`、`user_sessions`、`user_api_tokens`、`email_login_challenges`、`user_profiles` |
| 遗留 | `board_*`(接口已返回 410 Gone,数据保留) |

### 账户用量（Provider / Consumer）

账户页提供双视角用量（只计 model + token，不算 cost）：

- **Provider**：`owner_email` 下多 installation / share 的被消耗量 → `GET /v1/me/usage/provider`
- **Consumer**：`user_email` 跨 share 的调用量 → `GET /v1/me/usage/consumer`
- **公开 SVG**：`GET /v1/public/embed/global.svg`（全站）、`GET /v1/public/embed/usage/:username`（opt-in 个人）
- 数据来自现有 `share_request_logs` / `market_request_logs` 短窗口聚合（24h / 7d / 30d），见 `usage_account.rs` / `embed_usage.rs`

### 已知限制

- **单连接全局锁**:所有 store 方法(含只读)都要 `self.conn.lock().await`。WAL 对跨进程并发读有效,但不缓解进程内这把 tokio Mutex 的串行化。这是当前最确定的扩展性瓶颈。
- **`user_api_tokens.token_plaintext`**:默认 token 以明文存储,以支持在 UI 中重复展示 API key(`store.rs:12756, 23472`)。同表已有 `token_hash`。DB 泄露即等于活跃 token 泄露。
- 健康检查类表使用 AUTOINCREMENT 追加,依赖 `cleanup_expired_data` 清理而非 schema 层 TTL。

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
| `notification_task` | 5s | 离线/恢复邮件 outbox |
| `chat_notification_task` | — | 聊天邮件投递 |
| `client_market_trade_task` | — | 交易结算 |
| `ip_blacklist_log_task` | 600s | 黑名单统计落日志 |

**关停**:收到 SIGTERM 后先停 HTTP 接入并排空最多 30 秒,再关 SSH listener(5 秒)。

> **已知行为**:后台任务在关停时被 `.abort()`(`main.rs:492-500`),持有 DB 锁或处于事务中途的任务会被硬杀。

---

## 8. 通知系统

采用 outbox 模式,三阶段循环(`src/notifications.rs`):

1. **reconcile** —— 扫描在线状态,产出离线/恢复事件;推进持久化 baseline,使停用期间的事件不会补发
2. **aggregate** —— 同收件人事件在窗口内(默认 60s)合并为一个批次;风暴检测触发时进一步合并为 digest
3. **deliver** —— worker 以 90 秒租约认领批次,经 Resend 发送

可靠性设计:

- `FrozenEmailEnvelope` 保证重试时载荷不变,配合固定 Resend 幂等键
- 12 次尝试后进入死信
- 可重试状态码:408、425、429、5xx,以及带 `concurrent_idempotent_requests` 的 409
- 单收件人与全局双层小时配额,offline 与 registration 两条 lane 额度互不占用

---

## 9. 前端

Next.js 静态导出(`output: "export"`),`build.rs` 遍历 `frontend/out/` 生成 `include_bytes!` 匹配表编译进二进制,由 `/*path` catch-all 提供服务。单文件部署,无外部资源依赖。

- 无外部状态库,纯 React Context
- i18n 覆盖 `en` / `zh-CN`
- 设计 token 以 `--router-*` CSS 自定义属性承载,dark mode 经 `.dark` class 整体切换
- Web 终端:xterm.js ↔ WebSocket ↔ `portable-pty` 起 `ssh` 子进程(非 russh client),一次性 ticket + 每用户 2 会话上限 + 空闲/硬超时。**仅 Host Provider 本人可开**,租客与 Router 管理员均不可

---

## 10. 自升级

`src/admin/upgrade.rs` 实现进程自替换:

1. 同目录暂存临时文件(避开跨文件系统 rename 的 EXDEV)
2. 从 GitHub latest release 下载(180s 超时)
3. `chmod +x` 并以 `--help` 冒烟自检(5s)
4. SHA-256 比对新旧二进制
5. 原子 `rename(2)` 交换,旧二进制留 `.bak`
6. 探测服务管理器(systemd 或 nohup)
7. `setsid -f` 派生子进程延时重启

进度经 broadcast channel 以 SSE 流式推送至前端。
