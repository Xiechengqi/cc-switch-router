# Binance 自动到账运行手册

## 能力边界

该功能是“自动检测并确认到账”，不是代扣、代付或托管。买家仍在 Binance App 中主动向账单冻结的商家 UID 转账；Router 只用商家提供的只读 API Key 查询 Binance Pay 流水。首期只支持单 payment-home Region、USDT、15 分钟付款 intent 和精确金额自动匹配。

自动结算与供应商人工确认调用同一个账务事务：外部收据、账单 `paid`、余额清零、逾期限制解除和有效服务恢复要么全部提交，要么全部回滚。付款声明、争议、作废、商家轮换或停用凭据都会在同一事务中取消仍处于 pending 或迟到保护期的 intent。

## 安全模型

- 绑定前，商家必须先保存 6-20 位公开 Binance UID；API 绑定 UID 必须与公开资料完全一致。
- `/sapi/v1/account/apiRestrictions` 的全部已知安全相关字段必须存在。只允许 `enableReading=true`，提现、内部/通用转账、现货/杠杆、合约、Portfolio Margin、期权和 FIX 交易权限必须全部关闭；未来出现的未知 `enable*` / `permit*` 能力只要不是明确的只读能力，也会拒绝。
- 手工或周期权限复验一旦失败，账户会进入 `degraded`；手工失败还会清除权限验证时间戳。解密失败、凭据被拒、危险权限开启或 UID 不一致等硬故障会在同一事务中取消活动 intent 并冷却金额，后台必须先重新通过严格只读校验，不能沿用旧的 24 小时缓存继续结算。
- 新付款 intent 只会在 UID 已确认且权限校验仍处于 24 小时有效窗内时创建；长期没有自动付款的商家应先在账户页“重新验证”，避免买家打开收银台后才降级到人工付款。
- 商家主动绑定/复验按用户限制为 30 秒一次，避免认证接口被滥用拖累 Binance UID/IP 权重；生产边缘仍应保留常规的会话与来源 IP 限流。
- API Key、Secret 仅接受 16-256 字节可打印 ASCII。Secret 从不通过读取 API 或 UI 回显。
- 凭据与原始流水使用 XChaCha20-Poly1305 加密。凭据 AAD 绑定付款账户、商家用户和 credential revision；流水 AAD 绑定付款账户和 transaction ID，流水同时冻结当时的 UID、credential revision 与 key version，避免轮换后审计串账。付款方标识只保存主密钥 HMAC 指纹，避免数据库单独泄露后枚举低熵 UID。
- 首次绑定必须在最近 30 天历史中至少找到一笔能明确证明相同账户 UID 的 Binance Pay 流水（正向流水的收款 UID，或负向流水的付款 UID）；空历史或相关 UID 缺失时拒绝绑定，商家需先接收一笔受控小额转账再重试。这避免未证明账户所有权的用户抢占他人公开 UID。绑定完成后，个别流水缺少收款方 UID 时才可依赖已经确认的账户作用域、唯一金额和时间窗；明确返回不同 UID 的流水始终拒绝。
- 自动付款 UI 只使用账单冻结且与 API 绑定一致的 UID，不展示商家上传的二维码；Router 无法验证二维码内容，二维码只保留在人工付款资料中供用户自行核对。
- 同一 Binance UID 及同一 API Key 指纹在整个 Router 内只能绑定一次，避免同一笔账户流水在两个商家域中重复匹配。付款 home Region 是持久化账户身份的一部分；生产必须显式配置并保持稳定。
- 自动匹配要求：正向金额、USDT、`C2C`、唯一精确金额、交易时间位于 intent 创建前 120 秒至迟到宽限期内、账单仍可支付。
- 买家取消、改走人工付款、争议或作废后的金额仍轮询到迟到保护结束，到账只进入人工对账而不会自动改账；凭据轮换/账户停用产生的取消则不跨账户继续轮询。
- `transactionId` 在付款账户内唯一，外部收据对账单、intent 和流水均有唯一约束。并发轮询、重试以及人工/自动竞争不能重复结算。
- 数据最小化：付款、非 USDT、非正向或明确 UID 不匹配的无关流水只推进安全游标，不落库；正向 USDT 若与任何当前或历史 intent 均无关，只保留去重所需元数据，原始密文、订单号和付款方指纹会立即清空。

## 配置

生成独立的 32 字节主密钥，不要复用数据库 Token 或其他应用密钥：

```bash
openssl rand -hex 32
```

生产示例：

```dotenv
CC_SWITCH_ROUTER_BINANCE_AUTO_SETTLEMENT_MODE=shadow
CC_SWITCH_ROUTER_BINANCE_MASTER_KEY=<64-hex-characters>
CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION=1
CC_SWITCH_ROUTER_BINANCE_API_BASE=https://api.binance.com
CC_SWITCH_ROUTER_BINANCE_PAYMENT_HOME_REGION=region-a
CC_SWITCH_ROUTER_BINANCE_POLL_INTERVAL_SECS=4
```

生产 API base 只接受 `api.binance.com`、`api-gcp.binance.com` 和 `api1` 至 `api4.binance.com` 的标准 HTTPS 端口；任意第三方 HTTPS 主机都会在启动时被拒绝，防止配置错误把商家凭据发送到非 Binance 端点。HTTP/HTTPS loopback 仅用于本机测试。

主密钥必须由进程密钥管理设施注入，并与业务数据库分开备份。当前只装载一个 key version；轮换版本后，让商家重新绑定凭据。版本不一致时系统会在生成付款 intent 前拒绝；即使运维误换主密钥却沿用了相同 version，创建或刷新 intent 前的密文自检也会原子降级账户并取消活动 intent。系统绝不会回退明文或尝试错误密钥。

虽然流水接口是 Binance 的公开只读 API，把个人账户用于商业收款仍可能受账户所在地、KYC 类型和 Binance 条款限制。上线方必须自行确认授权与合规性；技术上的只读权限校验不等于 Binance 对商业用途的许可。

## 启用顺序

即使是全新数据库，也保留以下上线控制。它们不是为了旧数据迁移，而是为了隔离 Binance 上游字段变化、限流和真实资金匹配风险。

1. `disabled`：部署 migration、API 和 UI，但后台轮询、商家绑定/复验和买家 intent 都不会访问 Binance。服务启动时会在一个事务中取消遗留的 pending/expired intent、冷却对应金额、清除旧 poll lease，并持久化把全部绑定降为账户级 `shadow`；这也会阻止滚动重启中的旧 enabled 进程重新创建可付款 intent。随后 disabled 进程不再周期访问数据库；确认人工付款仍正常。
2. `shadow`：允许商家绑定和复验凭据，但这一阶段新保存的账户会被服务端强制保留为账户级 `shadow`，即使客户端请求 `enabled` 也不会预先激活；仅观察切换前已经存在的测试 intent/迟到保护流水，不修改账务，精确命中会把 intent 标记为 `review_required`、冷却该金额并进入管理员对账，避免买家继续看到“等待付款”。同一账单再次打开自动付款时只返回该待复核 intent，不会生成第二个金额；管理员忽略后才允许重建。首次实付验证应在隔离环境完成；生产若需验证，先在 `enabled` 下创建一笔受控 intent，切回 `shadow` 并重启后再付款。
3. `enabled`：总开关开启后，先让少量选定商家重新绑定/复验，使其账户级模式从 `shadow` 变为 `enabled`，再开放对应买家入口。观察至少一个完整的正常、过期、迟到和人工竞争场景后逐户扩大；切换总开关本身不会批量激活 shadow 阶段绑定的全部账户。

紧急停止只需把总开关改成 `disabled` 并重启。启动事务会立即取消已有活动 intent，并把账户级模式全部持久化降为 `shadow`；重新启用总开关后仍须逐商家重新绑定才能恢复自动结算。如果买家在关闭边界仍完成了转账，重新启用轮询后只会进入人工对账而不会自动改账。买家始终可以选择人工付款。单个商家可在账户页停用；删除操作还会清除加密凭据。

商家轮换 Key、修改绑定 UID、停用或删除凭据都会取消该账户全部活动 intent，并把此前由买家取消/刷新的迟到保护 intent 重新标记为账户边界取消，同时清空旧凭据的轮询游标；操作前必须确认没有买家正在付款。CAS revision 和 poll lease fencing 会丢弃在途旧凭据请求的结果，旧 worker 不能把另一账户域的流水写入或结算。

## 付款与尾差

账单 USD 分金额按 1 USDT = 1 USD 转成四位小数整数，再分配 `0.0001` 至 `0.0099` 的唯一尾差。每个付款账户、资产和实付金额在 pending 与 cooldown 期间唯一。付款、取消或过期后尾差至少冷却 24 小时，并且绝不早于 intent 的迟到保护终点。

单账单 24 小时最多分配 6 个金额，同一买家在同一商家账户最多 30 个，pending intent 30 秒内不能刷新。这些限制防止恶意刷新耗尽 99 个尾差。分配耗尽时系统明确降级到人工付款，不复用不安全金额。

普通重开收银台只会返回已有的 pending、expired、cancelled 或待复核 intent，不会静默换一个新尾差。只有买家明确确认“刷新金额”后才会生成新 intent；被替换 intent 会标记为 `buyer_refreshed`，旧金额即使在迟到窗口内到账也只进入管理员对账，不会自动结算。刷新前必须确认尚未按旧金额付款。

## 运维与对账

管理员市场账务页展示：

- open reconciliation case 数；
- pending intent 数；
- degraded 商家账户数；
- 最多 200 条按时间排序的待处理异常流水。

自动进入对账的场景包括金额无效、未知交易类型、未来时间、备注命中但金额不符、迟到/取消金额再次出现、候选歧义或账单已不可支付。错误 UID、非 USDT 和非正向流水直接忽略，不能由对账接口结算。

管理员“结算”仍会重新校验：正向 USDT、可接受的 ingestion 状态、目标账单与付款账户属于同一商家、存在 Binance intent、实付不低于基础金额且账单仍为 open/overdue。系统已经关联到账单的案例不能改挂到另一张账单，只有无关联案例才允许管理员输入目标账单。“忽略”与“结算”都会记录操作人、时间和 resolution；原始流水不在 UI 解密展示。

建议告警阈值：

- 任一账户连续失败 3 次进入 `degraded`；
- `degraded_account_count > 0` 持续 5 分钟；
- 最老 open case 超过 15 分钟；
- open case 或 pending intent 持续增长；
- Binance 返回 418、429、5xx 或 pagination limit。

## 故障处理

| 现象 | 处理 |
|---|---|
| `READ_PERMISSION_REQUIRED` | 开启读取权限后重新验证 |
| `DANGEROUS_PERMISSION_ENABLED` | 关闭交易、提现和全部转账权限；不要放宽 Router 校验 |
| `BINANCE_CREDENTIALS_REJECTED` | 轮换只读 Key；旧 Secret 不可取回 |
| `BINANCE_CLOCK_SKEW` | 校准 Router 主机时钟；在时钟恢复前保持自动结算降级 |
| `BINANCE_RATE_LIMITED` / `BINANCE_IP_BANNED` | 保持自动退避，检查同 UID 是否被其他系统高频查询；系统会采用 Binance 的秒数或 HTTP-date `Retry-After`，IP ban 无响应头时默认等待 1 小时，最长等待 3 天 |
| `degraded` | 查看稳定错误码和连续失败数；前端不会继续展示可转账指引。瞬时故障恢复并成功轮询后会自动回到 verified；硬性凭据/权限故障已取消活动 intent，须先修复并复验后再生成新金额 |
| 买家已付但未确认 | 不让买家重复支付；检查 intent、流水方向/币种/类型和管理员对账队列 |
| 金额池耗尽 | 使用人工付款，等待 24 小时 cooldown；排查异常刷新或同额高并发 |
| `CREDENTIAL_DECRYPT_FAILED` | 核对 master key/version；无法恢复旧 key 时让商家重新绑定 |

## 发布验证

```bash
cargo test binance_settlement -- --nocapture
cargo test
cargo clippy --all-targets
cd frontend
npm run typecheck
npm run audit:market-billing-contract
npm run audit:settings-contract
npm run audit:settings-i18n
npm run build
```

当前仓库旧模块仍有既有 Clippy 告警；发布检查不得新增 `src/binance_settlement/**` 告警。待全仓历史告警清零后，再把 `-D warnings` 恢复为统一硬门禁。

受控实付验证必须使用最小金额，并覆盖：正常精确到账、intent 边界迟到、重复流水、刷新旧金额迟到、人工声明竞争、停用账户和总开关停用。不要在生产日志或工单中粘贴 API Key、Secret、签名 URL或解密后的原始流水。
