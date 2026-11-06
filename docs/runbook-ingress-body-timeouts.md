# 入口请求体上传超时：迁移、保护与验收

## 修复范围

Router 在统一 API（`api.<domain>/v1/responses` 等）、直连 Share、Gateway、模型封禁前置检查和图片入口使用同一请求体 collector。固定 30 秒总读取时限会中断仍有进度的长历史上传；相同 session 重试仍上传大体量历史，因此可能持续失败，而新 session 较小可成功。这不意味着会话已损坏，也不能据此唯一确定网络底层瓶颈。

新实现允许持续有进度的上传超过 30 秒，同时保留首字节等待、数据空闲、总硬时限、路径大小上限及共享内存预算。入口失败发生在上游提交之前，不通过删除历史、裁剪工具结果、切账号或模型重试处理。Server/OpenAI OAuth 协议转换不需要为这次入口问题修改。

设计参考了 CLIProxyAPI 对未消费 HTTP/1 请求体的连接收尾，以及 codex2api 的共享请求内存准入和有界解压思路；实现使用本仓库的 Rust/Axum 数据路径，没有引入外部仓库运行依赖。OpenAI Docs 获取本次受到 HTTP 403 限制，本手册不把未取得的官方文档视为验证证据。

## 配置迁移（需要部署时执行，本次未修改生产）

建议部署配置：

```dotenv
CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_MAX_DURATION_SECS=300
CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_FIRST_BYTE_TIMEOUT_SECS=30
CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_IDLE_TIMEOUT_SECS=30
CC_SWITCH_ROUTER_PROXY_REQUEST_MEMORY_BUDGET_MB=256
```

`MAX_DURATION` 优先于旧 `CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_TIMEOUT_SECS`。旧变量仍保留原本的**绝对总时限**语义，不会悄悄改为空闲超时：如果生产配置显式旧值为 `30`，单独升级二进制仍会在总上传 30 秒时失败。必须显式设置新 `MAX_DURATION=300`，或删除旧值并采用新默认值。新配置生效后可清理旧变量；启动日志会提示仍在使用旧总时限的实例。

首字节/空闲范围 5–300 秒，总时限范围 5–3600 秒，内存预算范围 16–65536 MiB。Share `MAX_REQUEST_LIFETIME` 必须大于各阶段时限；上传和 Share 调度/响应是不同阶段。所有这些配置需要重启才生效。不要单纯把空闲超时调成数分钟来掩盖真正停止上传的连接。

大小默认保持普通 10 MiB、视频 32 MiB、图片 48 MiB。Gateway 按去除 `/_gateway/proxy/<share>/` 后的实际路径取大小上限。调高单请求上限时同时评估共享内存预算、Client 上限和 Cloudflare/其他网关的限制。

## 内存与压缩边界

- 上传按缓冲区实际容量准入；已知 Content-Length 预先准入，未知长度增长时同时计入旧、新分配的复制峰值。取消、超时、拒绝均通过所有权回收。
- `Bytes` 所有者携带内存 lease；路由重试、缓存、clone 和 reqwest 转发持有最后一份内容时仍计费。同一请求的前置检查和转发不重复读取、不重复记账。
- JSON 路由读取借用的 RawValue，不把整个历史扩展成 DOM；额外保守预留 `3 × decoded 长度 + 64 KiB` 工作区。图片 stream 请求借用保留原字段，仅重写既有的 `stream` / `response_format`，输出缓冲也计费。较大的合法请求仍可能因并发工作区不足返回 503。
- 仅在需要检查 model/stream 时解压；支持 identity、gzip、zlib deflate、zstd。压缩前后均受路径大小限制；不支持的编码（包括 br）、重复/多层编码返回 415，不盲目猜测。单个 zstd 解压窗口最多 8 MiB，保守预留 16 MiB decoder 工作集；gzip/deflate 预留 512 KiB。最多 4 个解压任务，快速拒绝，不排无限队列。
- 解压在 blocking 工作线程运行，5 秒总预算（包括队列等待），处理每个有界块检查 deadline。取消会中止尚未运行的任务；已经运行的原生调用不能强行终止，在有界输入、输出和窗口下收尾，其资源不会提前释放。
- 该预算是 Router 管理的请求缓冲与工作区预算，**不是进程 RSS 限制**，不覆盖 HTTP/TLS/SSH 栈、内核缓冲及其他业务状态。仍需进程级监控和容量规划。最小预算的实例可能无法接纳 zstd 工作集，应按实际负载配置。

常规转发保留原始压缩字节、Content-Encoding 和 Content-Length，IngressContext 继续签名原始实际发送内容。入站解压不是出站 Codex 压缩优化，也不是此次明文上传样本的已证实根因。

## 错误与观测

| HTTP | 阶段/原因 | 处理 |
|---|---|---|
| 408 | first_byte_timeout / idle_timeout / hard_timeout | 检查数据进度和配置；不切账号 |
| 413 | wire_too_large / decoded_too_large | 调整合理大小上限或减少业务负载；不要无限重试 |
| 400 | read_error / invalid_content_length / invalid_content_encoding | 检查截断、客户端关闭或压缩数据 |
| 415 | unsupported_content_encoding | 客户端使用受支持的单层编码 |
| 503 | memory_budget_exhausted / decode_capacity_exhausted / decode_timeout | 降低入口并发、评估预算；按 Retry-After 重试，不给 Share 熔断/健康记失败 |

响应的 `x-cc-switch-ingress-request-id` 由 Router 生成，不采信客户端自报的 ID。结构化 warning 记录 request ID、入口类别、阶段、声明/收到字节数、elapsed、last-progress-age、`upstream_sent=false`；只有对端属于可信 Cloudflare 网段才记录经过格式限制的 cf-ray，session header 只记录 SHA-256 截断摘要。不记录正文、提示词、凭据或任意底层错误文本。

统一 API 在 Share 选择之前失败，不会产生正常 Share 请求日志；应查 Router warning 和 live metrics，不能从 Share 日志缺少 408 推断“请求没有到 Router”。`router.proxyRequestBody` 提供各阶段计数、取消/容量/解压拒绝，以及当前/峰值/预算字节；监控页面显示这些 live 数据，历史 rows 无此新字段。

HTTP/1.x 未消费完 body 的超时或早退显式 `Connection: close`，避免残留请求体继续占用同一连接；HTTP/2 不发送 Connection 头，隔离到当前流。正常 chunked 上传和 keep-alive 仍支持。

## 本地回归与生产验收

本地回归不调用真实模型：

```bash
cargo test proxy_request_body::tests
cargo test proxy::tests::model_routing::request_body
cargo test
node scripts/audit/audit-settings-contract.mjs
node scripts/audit/audit-settings-i18n.mjs --check
npm --prefix frontend run typecheck
```

回归覆盖虚拟时钟的 80 秒持续上传、首字节/空闲/滴流硬时限、空帧/trailers、chunked、截断、路径上限、共享预算/并发/取消/clone/扩容峰值、有界解压和原始字节保留、压缩模型封禁、多个前置检查只读一次、HTTP/1.x 关闭及 HTTP/2 流隔离、同一 session 重试、入口错误不派发上游和不触发 Share 账号故障转移。

有部署授权后再进行生产验收：确认启动日志的生效时限和预算；复测原 session/长历史，不删除历史；确认连续上传超过 30 秒可成功、真正停顿仍被终止；对比 Router ingress request ID / 可信 Ray 与 Share 日志；观察入口超时分类、503 和内存峰值。若仍有 Cloudflare 408，应区分 Router 生成的 408 与更前置的边缘超时，不能宣称此次代码已排除所有网络问题。

此次不登录修改远程 Router、不部署、不重启，也不使用真实 OAuth 账号进行压测。

## 本地收尾 review（2026-11-06）

已复核上传时限、缓存复用、内存 lease 生命周期、取消与解压任务、签名原始字节保留、连接收尾，以及本地入口错误不触发供应商熔断/故障转移的边界。收尾补充了缓存解压结果遇到更小路径上限时的回归；CI 显式执行 collector、配置迁移、设置迁移和代理入口集成四组测试，均通过 `require-tests.sh` 拒绝零匹配。

本轮 `cargo test --locked request_body` 的 33 项测试通过（含 7 项代理集成），CI 脚本 6 项测试、前端 typecheck、设置契约与中英文审计通过。Clippy 完成，仓库仍有其他告警；本次 collector 中发现的三处可折叠条件已清理。

历史全量测试结果为 1923 通过、1 失败；本轮独立复现唯一失败 `model_price_catalog::tests::execution_snapshot_is_the_displayed_price`，价格快照为 2,121,714 字节，触发其半上限断言。价格目录源码与数据没有本次差异，未扩大任务范围修改定价契约，也未将本次验证表述为全量通过。远程验收仍需另行部署授权，并显式处理旧 `TIMEOUT=30` 配置。
