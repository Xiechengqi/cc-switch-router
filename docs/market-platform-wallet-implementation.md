# Market 平台钱包与 DujiaoPay 落地实施文档

> 状态：代码接线完成，待真实商户影子验收和生产切换。依据 Router `60cdcc0`、参考项目 `dujiao-next` `41b86260` 的代码审阅；若上游协议或 Router 代码变化，先更新契约测试和本文，再改资金路径。

当前实现进度：迁移 `0053`–`0062`，USD 整数复式账本、钱包冻结／捕获、DujiaoPay 签名回调与认证查单、充值补偿、七天收入成熟、线下提现／余额退款和 Owner 审批已接线。Share／Client 钱包合同具备试用、按秒批次、固定日历月履约、续费、暂停、确认清理后释放及服务争议退款；同一买家的到期服务优先于新单取款。旧财务写 API 在钱包模式返回 410，买家与 Provider 页面分别提供全站钱包及收入／提现入口。旧付费商品和报价保留原资金轨道，不能随模式切换自动升格；Provider 须明确重新发布。**生产支付尚未开放**：代码接线标志已打开，但必须经 Owner API 在旧义务与严重对账 case 清零、当前签名渠道完成真实入账验收后，才能写入 `cutover_approved_at` 并切换 `enabled`。验收查询只计入同时具有链上回执、已封口 journal 和对应钱包入账的 `credited` 订单；孤立的状态字段不能通过。仅手工改数据库 `mode` 不会开放新购买或普通用户充值。真实 USD 商户建单、签名回调、查单、链上回执和钱包入账尚无受控联调收据；本地 fixture 不能替代该验收。

> 2026-10 配置修订：迁移 `0056` 提供固定 DujiaoPay 回调 URL 和持久化查单时间表；迁移 `0057` 增加渠道版本锁、归档与密钥轮换状态；迁移 `0062` 将 Owner 的生产切换批准与模式字符串分开。DujiaoPay 官方控制台创建 Webhook 后会生成只展示一次的签名密钥；签名模式为默认，`poll_only` 是显式降级。代码集成不代表真实商户联调通过。

> 2026-10 充值闭环修订：迁移 `0067` 保存认证查单的脱敏付款标记、观测时间和摘要，不保存完整网关响应或交易 ID；`0068` 保存买家主动查单的排队、完成和重试状态。新充值操作不会按金额复用 `verifying/review_required/credited` 订单；但**同一个幂等键仍永远指向原订单**，建单结果不明时必须保留原键重试。所有付款入口先打开站内 `/wallet-checkout/` 页面，该页重新取得订单和认证查单的 `canPay`，确认没有付款线索、取消请求或过期网关期限后才跳往 DujiaoPay；不能凭旧列表里的链接继续支付。买家可以停止使用任一未结订单的站内付款入口，但这不是网关取消或资金结案。30 分钟是核实／尝试安全终结的时间，不是无条件关闭保证。

现有 `review_required` 订单不可通过迁移自动释放。Owner 应在「待核对充值单」中重新查单，检查脱敏付款标记、DujiaoPay 商户记录和本地回执／journal；只有网关当前明确为无付款证据的终态，且历史含糊线索获得商户侧核对记录时，才能使用现有带审计的恢复流程。若网关仍返回付款线索，继续隔离并升级人工核对；不要删除订单、手改状态或让浏览器回跳充当付款证明。核对单打开超过 24 小时且充值仍待核对的数量会显示在财务状态并写入告警日志。

## 1. 目标、决策与边界

目标是把 Share Market、Client Market 的付费流统一为「用户向平台充值 → 全站钱包支付 → Provider 按履约获得收入 → 沉淀至少七天 → Provider 申请提现 → Router Owner 线下付款」。用户不再给每个 Provider 单独充值，也不再为了租用一项服务配置某个 Provider 的收款信息。

已确定的产品决策：

1. 首版只有一个 USD 计价的钱包资产；同一用户在两个 Market 和所有 Provider 间共用。钱包归属使用 Router `user_id`，不使用邮箱、Provider 昵称或可变化的公开别名。首版不把其他游戏币或支付资产与 USD 混合；日后增加资产必须创建独立账本和明确兑换契约。
2. 充值渠道由 Router 内置。Owner 在 Settings 可配置并同时启用多个渠道，首个适配器为 DujiaoPay；多个渠道订单与密钥隔离，但成功充值进入同一个用户钱包。
3. DujiaoPay 首版使用托管收银台模式：Router 以固定商户订单号创建支付单，用户在收银台选择链和币种。Router 不要求用户输入 Binance UID，不伪造一个可由 Binance 自动识别收款人的二维码。固定链/币下单作为以后可加的渠道模式，不妨碍当前表结构。
4. Market 首版不继续向用户开放 Provider 专属预存、Provider 授信、账单付款声明或商家确认收款作为付费购买手段。免费供给、试用、商品准入、Provider Rank、服务控制和合同完整性校验仍保留。
5. 平台钱包扣款不是 Provider 立即获得可提现款。固定月租先记「未履约款」；按秒服务只对可计费履约时间扣款。形成 Provider 收入的每一份，最早在 `max(buyer_debited_at, fulfilled_at) + 604800 秒` 转为可提现。所有时间使用 UTC。
6. 首版平台佣金为零、用户充值面额等于钱包入账面额；网关费和线下转账费由 Owner 承担并单独记录，不隐性缩减用户余额或 Provider 应收。未来佣金或手续费必须按交易时的政策版本冻结并在报价中明示。
7. Provider 首版提现资产为指定网络的 USDT，面额按 `1 USD 账面金额 = 1 USDT 名义转账金额` 处理，网络费由 Owner 承担。Owner 线下实际转账，Router 不掌握出款私钥，也不调用 DujiaoPay 出款。其他币种、银行账户和动态汇率不是首版范围，扩展时须增加「报价—接受」步骤，不能把汇差藏进现有提现记录。
8. 用户可以对未消费、未冻结的钱包余额申请线下退款，采用与 Provider 提现相同的冻结、人工付款登记和审计原则。DujiaoPay 的返回页或 `refund.recorded` 事件都不等于 Router 已完成退款。
9. 不做旧财务数据迁移、不双写旧账本，也不把旧商家专属余额解释为新钱包余额。新资金域按新合同验收；旧迁移文件保持不可变，旧付费写入口在最终切换时退役。需要原位承接旧余额的运营环境不属于本计划。

## 2. 现状与替换清单

| 当前实现 | 代码入口／数据 | 新规则 |
| --- | --- | --- |
| 买家 × 商家预存与授信 | `schema/0049_market_prepaid_funding.sql`、`src/market_billing.rs`、`src/market_access.rs` | 钱包只按买家和资产分户；商家准入与资金账户脱钩。 |
| Binance 向商家 UID 充值及金额尾差 | `src/binance_settlement/`、`frontend/components/dashboard/market-funding-topup-dialog.tsx` | 改为向 Router Owner 的 DujiaoPay 商户收款，不复用 99 个尾差分配或商家凭据。 |
| 按秒服务从商家预存或信用扣费 | `src/market_billing.rs` 的 `allocate_accrual_funding_tx` | 从全站钱包预算与余额过账，Provider 收入按履约段生成。 |
| 固定月租首期和下一期均要求预留 | `src/market_recurring.rs` 的 `recurring_funding_summary_tx`、`capture_hold_tx` | 开通只预留／扣当期；下一期到期时再从全站钱包扣。 |
| 付费商品依赖 Provider 收款资料和结算资料 | `src/share_market.rs` 的 `ensure_payment_profile_tx`；`src/client_market_trade.rs` 的 `require_paid_offer_setup` | 发布时校验 Provider 身份与可用提现资料；收款资料不再面向买家。 |
| 每 Provider 资金汇总 | `src/market_providers.rs`、`docs/provider-ranking.md` | 买家侧改为一份全站钱包；Provider 页保留供给和质量，私有区展示与该 Provider 有关的订单／消费而非分户预存。 |
| 旧账单、退款义务和 Provider 人工确认 | `src/market_billing.rs`、`frontend/components/dashboard/account-billing-page.tsx` | 新账本、用户余额退款与 Provider 提现替代付费主路径；旧业务 API 退役时明确返回 `410`，不得落到泛用代理路由。 |

`dujiao-next` 可参考的代码路径：

- `internal/modules/payment/infrastructure/gateway/dujiaopay/dujiaopay.go`：API HMAC、幂等建单、Webhook 原始 body 验签。
- `internal/modules/payment/infrastructure/gateway/adapters/dujiaopay/adapter.go`：渠道配置及托管收银台适配。
- `internal/modules/payment/application/payment_service_recharge.go`、`payment_service_callback_wallet.go`：先建本地充值单，之后网关调用；成功时在同一事务更新支付单和钱包。
- `internal/modules/payment/transport/http/admin_channel_handler.go`：管理 API 遮盖密钥的交互。
- `internal/modules/payment/application/payment_service_wallet_test.go`：迟到、重复和并发回调的测试思路。

不得直接复制参考项目的资金判断：当前参考实现对签名正确的 `order.paid` 直接标为成功，未检查 `paid_source`；其钱包金额按两位小数处理，Router 现有按秒计费使用 `MONEY_UNITS_PER_MINOR = 86400`；渠道密钥遮盖也不等于数据库密文存储。此外，参考网关错误可能携带原始响应 body，Router 的公共错误和日志必须只保留稳定、脱敏的错误码。

## 3. 核心不变量与金额精度

- 每笔账务事件由一笔不可变 journal transaction 和至少两笔 posting 表达；同一 journal 内、同一资产的带符号 posting 之和恒为零。修改历史只允许新建冲正或调整流水，不 `UPDATE` 已记账金额。
- 用户钱包 `posted_units >= held_units >= 0`，`available_units = posted_units - held_units`；hold 的建立／释放要在 journal 内表达用户可用与冻结逻辑账户之间的转移，缓存与 hold 明细同事务更新。扣费、退款和提现申请均在 `IMMEDIATE` 事务内用条件更新或等价 CAS 防止超支。
- 保留 Router 现有整数精度：`1 USD cent = 86400 money units`。DujiaoPay 的 USD 十进制字符串必须先精确解析为美分，再乘以 86400 入账，例如 `1.00 USD = 100 × 86400 = 8640000 units`；拒绝超过两位小数或溢出的面额。按秒费率用整数及 `i128` 中间计算。任何前端展示取整都不能反向影响账本。提现和线下退款只转出完整美分；不足一分的剩余单位留在原账户，不能抹掉。
- 充值、合同开通、按秒扣费、月租续费、服务退款、收入确认、收入解锁、提现申请和付款登记分别使用独立的、带主体作用域的幂等键。相同键加相同请求指纹返回旧结果；相同键加不同指纹返回冲突。
- 任何 Provider 收入必须引用一笔已入账的买家扣款以及可核对的 Share／Client 合同与履约证据；绝不能仅凭「服务处于 active」或「充值成功」生成 Provider 收入。
- 任何 Provider 的可提现额不得超过其已确认履约收入减去冲正、退款、争议冻结、已申请提现和已付款。用户钱包余额、未履约款、Provider 待解锁／可提现／提现中金额与平台费用必须能逐笔从 journal 重算。
- 公开 API 不返回钱包余额、合同财务明细、提现地址、支付商户密钥或真实用户身份；资金 API 一律按 Session user ID 或 Owner 权限过滤并设置 `Cache-Control: private, no-store`。

首版 journal 是 **USD 计价的资金义务子账本**，不是 DujiaoPay 收到的链上币种库存账。约定 journal 的借方为正、贷方为负；负债类账户在贷方增加，余额投影则以正数展示。`Owner 清算桥` 是每笔入金、出金的 USD 计价对手账户，不宣称 Owner 实际持有等量 USD；真实收币、换币、费用和出币须用外部流水另行对账。最低过账模板如下，所有行必须同事务写入 journal、幂等来源和对应投影：

| 事件 | 借方 | 贷方 |
| --- | --- | --- |
| 已核验的充值 | Owner 清算桥 | 买家钱包可用负债 |
| 购买冻结／解除冻结 | 买家钱包可用／冻结负债 | 买家钱包冻结／可用负债 |
| 月租当期捕获 | 买家钱包冻结负债 | 月租未履约负债 |
| 按秒服务捕获 | 买家钱包冻结负债 | Provider 待解锁负债 |
| 月租履约片段 | 月租未履约负债 | Provider 待解锁负债 |
| 七天成熟 | Provider 待解锁负债 | Provider 可提现负债 |
| 提现申请／拒绝退回 | Provider 可提现／提现冻结负债 | Provider 提现冻结／可提现负债 |
| 提现线下付款登记 | Provider 提现冻结负债 | Owner 清算桥 |
| 用户余额退款申请／拒绝退回 | 买家钱包可用／退款冻结负债 | 买家退款冻结／钱包可用负债 |
| 用户余额退款线下付款登记 | 买家退款冻结负债 | Owner 清算桥 |

服务退款以原收入所在的未履约、待解锁、可提现或提现冻结账户为借方，以买家钱包为贷方；先冻结争议份额，再在不超过尚未线下付出金额的范围内冲正。已经实付的部分另记 Owner 对 Provider 的追偿或损失，不允许把原提现 journal 改写成未付款。`posted_units` 是该用户钱包可用与冻结负债的合计正数投影，移动 hold 不改变它。

## 4. 数据模型与迁移

从 `schema/0053_market_platform_wallet.sql` 开始新增迁移，并在 `src/schema.rs` 登记。名称仅作为实施建议；若实施前已有新迁移，以届时下一个版本号为准。旧 migration 不改写。先以外键、`CHECK`、唯一约束和索引表达可落在数据库层的约束；业务事务再校验跨表守恒。

| 新表／对象 | 必备字段及约束 |
| --- | --- |
| `market_wallet_accounts` | `user_id, asset_code` 唯一；`posted_units, held_units, version` 非负；首版只允许 `USD`。 |
| `market_journal_transactions`、`market_journal_postings` | 全局唯一 `source_kind, source_id, action` 或等价幂等键；posting 含逻辑账户、主体、资产、带符号整数单位、创建时间；journal 提交前验证同资产净额为零。 |
| `market_wallet_holds` | 用户、合同／报价／退款来源、金额、用途、状态、到期时间、版本；同一业务来源只允许一个活动 hold。 |
| `market_payment_channels`、`market_payment_channel_revisions` | Owner 配置、排序、启用状态、网关类型、商户身份指纹、配置代次；API Secret 加密保存，Webhook Secret 可选，revision 明确记录 `signed` 或 `poll_only`。读 API 不回显密钥。 |
| `market_topup_orders`、`market_topup_events` | 用户、渠道与配置代次、商户单号、网关订单号、USD 面额、状态、过期／到账时间；商户单号全局唯一、`商户身份 + 网关订单号` 唯一、`商户身份 + 网关事件 ID` 唯一。链上回执的网络、币种、交易 ID、确认数和结算金额须有可核对快照；同一真实收款证明不能被两笔订单复用。事件保存必要审计字段和原始 body 摘要，不向普通日志复制原文。 |
| `market_delivery_slices`、`market_provider_earning_lots` | 合同、服务有效时间段、原始计费来源、扣款 journal、Provider `user_id`、收入单位、冲正单位、`fulfilled_at`、`unlock_at`；来源片段唯一。月租片段另引用期间 ID。 |
| `market_usage_posting_batches` | 按合同和有界时间窗归并已持久化的按秒 accrual；起止游标、总单位、journal ID 唯一，便于补偿重放。 |
| `market_payout_destinations`、`market_payout_requests`、`market_payout_allocations` | Provider 归属、网络与地址、地址版本／快照、申请金额、状态、Owner、外部转账流水号；一份 earning lot 不可超额分配。 |
| `market_buyer_refund_requests` | 用户、钱包 hold、金额、收款地址快照、状态、Owner、外部转账流水号；只能冻结可用余额。 |
| `market_finance_reconciliation_cases`、`market_finance_audit_events` | 异常代码、对象、处理状态、操作者和时间；不得把 Secret 或完整支付 body 写入审计 JSON。 |

逻辑账户至少有：Owner 收款待清算、用户钱包负债、用户钱包冻结、月租未履约款、Provider 待解锁应付、Provider 可提现应付、提现冻结应付、平台费用／损失及异常待核对。余额缓存是 journal 的投影，必须有可独立重算的对账器；不能把缓存作为唯一资金证据。

Provider 金融归属固定为交易发生时的 `supplier_user_id`，公开 `market_provider_id` 仅用于展示和身份发现。Provider 换邮箱、改展示名或发生公开身份合并，不能重指向历史提现权；身份冲突时停止新收入释放和提现，进入 Owner 对账。

## 5. DujiaoPay 渠道契约

### 5.1 配置与启用

Settings 的 Marketplace 分类提供独立「充值渠道」面板及专用 Owner API。每条渠道可设置展示名、API Key ID、API Secret 和 Webhook 签名密钥；保存时先用 `whoami` 验证商户 API，渠道初始关闭。固定回调地址 `https://<Router 公共域名>/v1/payment-webhooks/dujiaopay` 在建渠道之前即可复制并配置；不能要求先取得渠道 ID。实际地址由 Router 可信域名配置生成，不接收浏览器提供的目标 URL。API Base 在正式环境固定为受信任的 HTTPS 官方主机，仅测试允许 loopback。

适配器实现 `GET /v1/whoami` 商户身份验证、`POST /v1/orders` 建单及 `GET /v1/orders/{id}` 查单。API 请求使用独立 API Secret 签名。DujiaoPay 官方控制台创建 Webhook 后生成独立签名密钥，只显示一次；不能填假值或复用 API Secret。新渠道默认 `signed`，使用原始 body HMAC 验签；无密钥时只有 Owner 明确选择 `poll_only` 才能配置：无签名回调只可按已存在的随机商户单号唤醒查单，不能直接改变订单状态或入账。既有 revision 迁移后默认保持 `signed`，原带渠道 ID 的回调地址继续有效；替换既有签名渠道时不允许默默清空 Secret 或降级。具体字段、`paid_source` 枚举、USD 商户支持及回跳 URL 仍须以真实商户受控联调锁定，不能只凭参考项目 fixture 声称已验证。

启用顺序：先在商户后台设置固定回调 URL 并保存只显示一次的签名密钥 → 保存加密的 API／Webhook 配置并通过 `whoami` 冻结商户身份 → 用受控最小金额真实 USD 收款验证签名回调、建单、查单、链上回执及钱包入账 → Owner 明确启用。`whoami` 成功只代表 API 凭据有效，不代表 Webhook 或真实充值已验收。资料编辑只递增渠道版本；API 与 Webhook 密钥轮换各产生新 revision、停止新单且不得跨商户。未终结订单保留旧 revision 的交易身份及允许支付方式；已知网关订单优先用同商户当前 API 密钥查单，失败时只读回退旧密钥，以兼容新密钥吊销旧密钥或网关按密钥隔离历史订单。创建结果不明的旧订单只用原建单密钥重试同一商户单号，避免跨密钥重复建单；若原密钥已失效，须人工对账。旧 revision 仍须可解密；无法解密时停止自动入账并告警，不降级明文或信任浏览器状态。

Owner 若确需把既有签名渠道改为仅查单，须使用更换回调密钥 API 显式提交 `webhookMode: "poll_only"`；只省略 Secret 会被拒绝。恢复签名模式须提供新 Secret 和 `webhookMode: "signed"`。签名验签使用同一商户渠道的当前回调密钥，不依赖旧订单的 API revision；普通轮换可短暂接受前一个签名密钥 10 分钟，紧急轮换立即撤销。降级 `poll_only` 后，固定及旧带渠道 ID 的回调都只能唤醒认证查单。归档只停新单，不删除渠道或订单；归档也立即撤销上一回调密钥的过渡期，旧订单仍由当前签名密钥或认证查单处理。恢复后仍保持停用，并须通过 Owner 的 `verify` API 重新验证现有商户 API，或轮换为同商户的新密钥，才能再次启用。归档／恢复也清除旧回调验证状态，新的签名回调需重新证明。固定地址可接受不含订单号的商户后台测试通知，但不会将其视为支付证据或签名验证通过。

影子模式允许 Owner 为验收暂时开启已验证 API 的渠道；一旦全局财务模式为 `enabled`，停用渠道重新启用还必须有当前 revision 的签名回调验证和真实核验入账。密钥轮换后若缺少这些证据，应先回到影子模式完成受控充值验收，不得绕过验收直接恢复生产接单。

### 5.2 充值状态机

`created → creating → pending → verifying → credited` 是正常路径；另有 `create_unknown`、`expired`、`cancelled`、`review_required`。`credited` 为终态，不允许后到的过期或取消回调降级。`expired/cancelled` 后真实链上到账不能丢失：严格匹配成功则入账一次，无法严格匹配则进入人工核对。`create_unknown` 表示建单请求超时且不能证明上游没有建单，必须以原 `merchant_order_id` 和相同请求体／幂等键重试，不能生成第二个应付订单。

创建顺序：

1. 鉴权、限流、校验 USD 金额上下限与渠道启用状态；用浏览器幂等键和请求指纹在本地事务中先写订单及配置 revision。金额一经创建不变。
2. 事务外签名调用网关，优先选择托管收银台；仅保存同一订单的 `order_id`、`checkout_url` 与过期时间。返回的收银台 URL 必须为允许的官方 HTTPS origin，拒绝任意跳转地址。网关调用绝不在 Router 数据库 mutex／事务内进行。
3. 若网关调用超时或更新本地订单失败，保留 `create_unknown`，由补偿任务用同一商户单号重试／查单。Webhook 可能先于建单响应到达，因此应能先按已存在的 `merchant_order_id + channel_revision` 定位本地订单，待网关 ID 补齐后再次核对。
4. 浏览器回跳只显示「付款核对中」；状态由 Router 私有查询 API 获取，绝不由 `success_url`、前端轮询次数或截图决定入账。

固定 URL 对无签名渠道只解析格式受限的 `merchant_order_id`，用本地唯一订单绑定找到原渠道 revision，并在持久化的单订单五分钟冷却下提前安排一次查单；不保存通知宣称的支付状态、金额或网关订单 ID，也不由请求线程直接查询网关或记账。签名渠道使用当前或短暂过渡的回调密钥 HMAC 校验和事件审计，固定 URL 与旧带渠道 ID 的 URL 均可使用。两种模式真正入账均依赖事务外 `GET /v1/orders/{id}` 的认证响应，并在 `IMMEDIATE` 事务内核对渠道／商户、双重订单 ID、USD 面额、`paid_source=chain`、链上回执和唯一性。网关暂不可用时保留持久化重试时间，不能把通知当作收款证明；证据冲突则进入 Owner 复核。`refund.recorded` 只作为外部记录信号，不自动扣除钱包或认定线下退款完成。

回调路由无用户 Session；固定 URL 的无签名内容始终不可信，限流、64 KiB body 上限、单订单冷却和最多 24 单／轮查单预算是其资源边界。所有未入账状态按年龄退避，并在创建后 90 天内定期查单；窗口结束后仍可被回调唤醒。超过窗口且没有回调的迟到付款不能自动发现，须以 Owner 对账处理，不能宣称零漏账。全局 IP blacklist 对两个精确回调路径豁免，但不豁免限流或签名渠道验签。不得在日志、前端错误或告警正文输出 Secret、签名头、原始失败 body 或完整支付资料。

无签名通知中的退款字段同样不能直接认定为退款，也不会生成可信退款审计；Owner 必须用商户后台与外部收款流水独立对账。

超过 90 天的旧订单收到通知后仍会开启最长六小时的持久化查单恢复窗：网关暂不可用时五分钟后重试，未付款则限频复查，窗口结束后停止。无通知且无外部对账时无法证明“绝无漏账”。

## 6. Market 扣费、履约与退款

### 6.1 报价及购买

Share 与 Client 的 quote 返回不可变报价快照：商品和 Provider `user_id`、商品修订、价格、计费模型、有效期、钱包可用额、被已有服务占用额、当前订单最小差额。新订单提交前仍复核在线／库存／授权／价格／身份和报价指纹。

钱包不足时显示「充值差额」，保留商品上下文；充值到账后重新报价，用户再明确确认，不自动复用过期报价或自动下单。报价查询不冻结余额。提交时在同一 `IMMEDIATE` 事务内冻结开通所需金额并创建待激活合同；Provisioning／Share 授权成功时捕获当期款。失败时必须先确认授权／Host 清理已完成，才能释放全部 hold；外部状态不确定时保留冻结并进入恢复或人工复核，不能退款后留下仍可使用的服务。相同报价重试不会重复建服务或扣款。免费和试用订单不创建付费 hold。

### 6.2 按秒服务

现有 Market reconcile 频率较高，不应把每次健康检查都扩展为多笔复式财务流水。保留逐段服务间隔／accrual 作为履约证据；以有界批次（目标约一分钟，具体用性能测试定上限）按合同、账期顺序汇总并过账。批次包含闭开时间窗与最后处理的 accrual 游标，重试不重复扣款。活跃服务从钱包预留约 24 小时的预算，已激活服务的预算优先于新订单；每次批量过账同时减少 posted 与相应 hold，再尝试补足滚动预算。余额不足时只记已被资金覆盖的履约时间，按现有控制链路暂停对应服务，不能产生平台负余额或隐形 Provider 信用。

当多个服务共用钱包且不足以全部补足时，先兑现现有 hold，再按到期时间、既有 `renewal_priority`、合同 ID 的稳定顺序分配；结果必须可重放、可审计，并在用户界面标出可能暂停的合同，不能让 HashMap 遍历顺序或 worker 抢锁顺序决定谁先得款。

### 6.3 固定日历月租

开通或续期只扣当前服务期款，不再强制预留下一整期。自动续期到期时以当时的钱包余额原子扣款；不足则进入现有 recovery／暂停控制链路，提前提醒但不静默借用 Provider 授信。试用转付费按实际开始时间建立第一服务期。用户选择「到期取消」则不扣下一期。

当期扣款进入「未履约款」，不立即生成 Provider 可提现额。以半开服务区间 `[period_start, period_end)` 的可用服务时间生成履约片段，按 `i128` 整数比例分配当期总单位；最终片段吸收整数余数，保证片段总额、自动退回金额与当期扣款完全相等。这里的履约指合同约定的可用服务，不要求用户必须向模型发起请求。可核实的停服／合同完整性故障不计 Provider 履约；未履约余额按现有合同规则退回用户钱包，期末不能永久挂在未履约账户。Provider 提前终止、开通失败、价格变更和争议均先冲正未解锁收入，再决定是否冻结已解锁但未付款收入。

### 6.4 退款、争议和收入逆转

服务退款回原平台钱包，不回某个 Provider 专属账户；同时冲减同一合同的未履约款或未付 Provider 应收。争议发生时，冻结相关 earning lot、暂停提现申请；确认后以新的冲正 journal 调整，不修改历史 lot 金额。若对应款项已进入提现或线下转账状态，须先确认外部转账未发生，才可从提现冻结款退款；转账结果不明时保持隔离待对账，不能同时退款和继续付款。若对应款项已线下转出，则记录 Provider 应收回款或 Owner 损失，不能制造负用户钱包，也不能用别的 Provider 余额掩盖。用户未消费余额退款只冻结当时的可用余额；申请批准后 Owner 转账并录入唯一外部流水号，失败／拒绝且确认未出款后才释放冻结。

## 7. Provider 收入与线下提现

每笔收入 lot 固定 `buyer_debited_at`、`fulfilled_at`、`unlock_at`、Provider `user_id`、合同和扣款来源。定时任务仅把未争议且未冲正的成熟份额从「沉淀中」转到「可提现」；重复运行、时钟回拨和服务退款并发不能重复解锁。月租首日收到的钱最早是首段服务履约完成七天后逐步解锁，不是整月扣款后七天一次性解锁。

Provider 在账户页保存网络、资产和地址；地址变更需二次确认、版本化并有 24 小时冷静期，已提交提现申请永远引用旧地址快照。提现申请从可提现 lot 按 `unlock_at, lot_id` 稳定顺序分配，原子转为冻结；重复点击返回同一申请。Owner 审核后线下转账，先把申请从 `approved` 标为 `transfer_in_progress` 并锁住退款／拒绝操作，再填写链上交易 ID、转账时间、资产、网络和实际到账名义金额；系统核对名义额与申请额及外部流水号唯一性才标记 `paid`。正常路径为 `requested → approved → transfer_in_progress → paid`；仅确认**未发生外部转账**时，才可从 `requested/approved` 拒绝并解冻，或把失败转账转为 `transfer_failed` 后重新批准／拒绝。结果不明进入 `review_required`，保留冻结，不可自动重发或解冻。每一步都记录操作者与版本；`paid` 不允许撤回或再次付款。Router 不能自动确认外部链上到账，因此「Owner 标记已付」是人工审计声明，UI 要明确这一点。

### 7.1 运营方偿付与对账

Owner 收到的是 DujiaoPay 商户实际入金资产，Router 给出的却是 USD 计价钱包负债；两者不能因一笔网关回调就被视为同一资产。必须每日对照网关已付款订单、收款账户流水、用户钱包负债、月租未履约款、Provider 未付款应付、用户退款冻结和 Owner 已转出凭证。网关费用、资产转换损益和转账网络费独立入账，不改用户或 Provider 名义金额。对账差额、缺少外部付款凭证、已付标记但未知转账流水、已收款但未入账、负值或跨主体引用均创建 Owner 待处理 case 并暂停受影响渠道或提现。

## 8. API 与页面合同

建议 API（实施前以现有路由和类型命名做一次冲突审查）：

| 范围 | API | 关键约束 |
| --- | --- | --- |
| 用户钱包 | `GET /v1/market-wallet`；`GET /v1/market-wallet/transactions` | Session 归属，游标分页，私有禁缓存。 |
| 充值 | `GET /v1/market-wallet/channels`；`POST /v1/market-wallet/topups`；`GET /v1/market-wallet/topups/:id` | 创建使用 `Idempotency-Key`；只返回必要的安全收银台 URL 和状态。 |
| Webhook | `POST /v1/payment-webhooks/dujiaopay`；旧 `/:channel_id` | 固定 URL 无签名渠道只唤醒查单；旧 URL 和签名渠道保持原始 body 验签，入账始终以认证查单为准。 |
| 用户退款 | `POST /v1/market-wallet/refund-requests`；`GET /v1/market-wallet/refund-requests` | 仅可用余额，冻结地址和金额快照。 |
| Provider 收入 | `GET /v1/market-provider-finance/me`；`GET /v1/market-provider-finance/earnings` | 待解锁、可提现、提现中分别展示；不得混作用户钱包。 |
| Provider 提现 | `PUT /v1/market-provider-finance/destination`；`POST /v1/market-provider-finance/withdrawals`；`GET /v1/market-provider-finance/withdrawals` | 收款资料版本化，提交幂等。 |
| Owner 渠道 | `/v1/admin/market-payment-channels` 下创建、资料编辑、现有 API 复验、API／Webhook 密钥轮换、启停、归档／恢复接口 | Owner-only；修改使用 `expectedVersion` 乐观锁，读响应不回显 Secret；归档后旧订单仍可对账。 |
| Owner 财务 | `/v1/admin/market-finance` 下模式、暂停、充值异常、提现、用户退款、对账 case 查询与处置接口 | `PUT /pause` 独立控制新钱包订单和新提现；所有操作有操作者、理由、版本 CAS 和审计事件。 |

用户界面只有四个购买重点：商品与周期价格、全站可用余额、需充值差额、主操作「确认租用／充值差额」。不显示 Provider UID、尾差、账单声明或多个分割区。充值结束返回原商品卡并刷新报价；未成功就保持「等待到账／人工核对」，不显示假成功。账户新增全站钱包和充值流水；Provider 收入／提现独立面板；Settings 的 Marketplace 分类增加 Owner 充值渠道与财务状态面板。原 Provider 页中预存／信用汇总与按 Provider 充值入口删除或替换，Provider 排名算法及公开质量数据不受钱包改造影响。

## 9. 安全、异常和停机策略

- DujiaoPay API/Webhook 密钥使用独立于 Binance 的主密钥和 AEAD 加密，AAD 绑定渠道 ID、商户身份和 revision；密钥文件／环境 Secret 与数据库分开备份。错 key 或 key version 必须 fail closed，不回退明文。参考 `src/binance_settlement/crypto.rs` 的封装思想，但新支付模块使用独立密钥域。
- 只有 Owner 可改渠道、审批出款或处理人工入账；「人工确认已付款」必须写明外部证据、金额、来源和理由，不能仅凭签名 `order.paid` 中的人工 `paid_source` 自动记账。审计可以记录摘要／稳定错误码，不记录签名头、API Secret 和原始失败 body。
- 财务模式有 `disabled / shadow / enabled` 切换门禁及单渠道开关。`shadow` 仅允许 Owner 测试充值；`enabled` 仅在 Owner API 通过真实入账和切换门禁后开放，批准后不可退回旧支付轨道。生产异常使用独立的「暂停新钱包订单／新提现」开关；关闭渠道可停止新充值。暂停不阻止既有合同履行、已发起转账的结果登记、用户余额退款、Webhook 或对账，也不抹掉 pending 充值订单；已经付的钱仍须查单和核对。配置页明确区分 API 验证、签名回调验证及当前 revision 的真实入账，不能把空订单号测试投递误记为通过。
- 支付网关或 Turso 不可达时停止新建单／扣费的相应写入；签名 Webhook 事件和无签名回调唤醒的查单时间表持久化，网关恢复后重试。数据库事务内不发外部 HTTP。Router 当前 Turso 模式仅支持一个可写实例，本计划不引入多主写入或离线写。
- 设定金额上下限、用户及 IP 建单速率、单用户 pending 订单上限、Webhook body 上限、外部 HTTP 超时与退避、健康告警和补偿任务预算。无渠道可用时明确提示，不能退回旧商家直付链接。
- 对账异常、Secret 解密失败、网关身份变化、连续查单失败、零和不变量破坏、重复外部转账参考号等需要可观测的 Owner 告警。资金不变量失败时冻结受影响资金路径，而不是尝试用调整 SQL 自动修复。
- 投产前冻结每个渠道经真实商户验证的最小／最大充值额、同时待付订单数、建单节流和提现最低额；不得把网关不支持的低额订单开放给用户，也不得以默默提高付款额的方式满足渠道下限。地址冷静期和七天收入沉淀是独立计时器，不能互相替代。

## 10. 实施分期与每期完成条件

各期可拆成独立 PR，但新旧财务写路径不得同时对同一笔业务生效；前几期功能保持内部关闭，最终统一切换。预计完整范围为 8–12 工程周，真实商户联调、异常对账和性能结果可能延长时间。

主要代码落点建议如下；先抽出资金原语和网关适配，不继续扩大已有大型 `market_billing.rs`：

| 工作包 | 新增／修改位置 |
| --- | --- |
| 数据与装配 | `schema/0053_*.sql` 起、`src/schema.rs`、`src/server_state.rs`、`src/api.rs`，新增 `src/market_wallet/`、`src/market_payments/`、`src/market_payouts/`。 |
| 购买与履约 | `src/market_billing.rs`、`src/market_recurring.rs`、`src/share_market.rs`、`src/client_market_trade.rs`；只通过共享资金原语记账。 |
| Provider 与旧入口 | `src/market_access.rs`、`src/market_providers.rs`、`src/binance_settlement/` 及其路由；付费主路径退役后旧模块只保留必需的历史读取或明确 `410`。 |
| Owner 配置 | `src/admin/settings.rs` 的 Marketplace 入口、专用渠道和财务 API、加密密钥配置及告警接线。 |
| 前端 | `frontend/lib/api.ts`、`frontend/lib/types.ts`、`frontend/lib/i18n.ts`、Settings 渠道面板、Share／Client 租用弹窗、账户账务页和 Provider 页面；更新对应组件及契约测试。 |
| 文档与审计 | `docs/provider-ranking.md`、新运行手册、现有 `scripts/audit/` 的 Market／Settings 合同、网关 mock fixture；不让旧词条伪装成仍可用能力。 |

1. **契约冻结与测试基座**：冻结金额精度、订单／收入／提现状态机、DujiaoPay 签名和查单 fixture；完成钱包、Provider 身份和商品原有行为基线测试。明确实际商户是否支持 USD `fiat_currency` 和 `paid_source` 可查。未证明则首版渠道不得启用；不要静默转用 CNY 汇率。
2. **新账本与迁移**：新增新表、journal 原语、钱包 hold、缓存重算器、并发及幂等测试；添加 `src/schema.rs` migration，不能改历史 schema 校验和。通过零和、非负、重复重放、崩溃恢复及本地／Turso 测试后进入下一期。
3. **DujiaoPay 入金**：新增独立网关适配和渠道配置／加密存储、签名建单、回调、查单、补偿、Owner 配置面板。用 mock 网关覆盖全部失败分支，再以受控最小金额实测；只有实测通过才可以开放给真实用户。
4. **Share／Client 消费接线**：报价和提交改读全站钱包，开通 hold、失败释放；将按秒 accrual 批量过账；将固定月租当期扣款、续费及提前退回接到未履约款。删除付费购买对 Provider 收款资料、专属预存和授信的依赖，同时保留服务控制与试用逻辑。
5. **收入与出款**：从履约证据生成 earning lot、七天解锁、争议冻结／冲正、Provider 收款地址与提现流程、用户未消费余额退款、Owner 付款登记和每日对账告警。
6. **界面和旧入口退役**：简化购买弹窗和账户页，Provider 页改全站钱包口径，移除旧 Binance 专属充值入口；旧资金写 API 明确退役，不能被遗漏的前端调用重新打开。更新 API 合同／前端类型、i18n、`docs/provider-ranking.md` 与运行手册。
7. **发布验收**：先 disabled 部署 schema 和 API，shadow 完成受控网关联调与内部对账，再对少量新账户 enabled。观察至少一次正常充值、迟到充值、重复 Webhook、未付超时、月租退款、按秒资金耗尽及完整的七天提现解锁。出现金额不变量异常立即关闭新建单与新提现，保留 Webhook 和已收款对账能力，不能简单丢弃回调。

发布前应备份新账本数据库与独立密钥，并演练从备份重算 journal 投影。**第一次真实充值后不能回滚到不认识平台钱包的旧二进制**：旧版不会扣新钱包、处理新 Webhook 或守住 Provider 提现边界。故障处置是关闭新订单和提现、继续接收／持久化回调并向前修复；只有在没有任何真实新账本资金或合同之前，才允许完整回退到旧版本。此限制与“不做旧数据迁移”是两个不同问题。

**无旧数据迁移的切换门禁**：最终打开新付费写路径前，自动出具旧域未清清单，至少覆盖进行中的按秒／月租合同及续期、开通或释放中的授权、旧预存余额与 hold、授信欠款、未结账单、退款义务、待确认 Binance 充值和未完结争议。清单非零时不得把这些合同指向新钱包、直接删除旧表或关闭其必要的控制／退款处理；必须先在旧路径完成服务终止和资金／义务清结，再切换，或在一个没有旧付费义务的全新实例启用。旧的已发布付费商品先从可购买目录隐藏，Provider 按新规则重新发布后才重新开放，不能自动把旧报价认作新钱包报价。这个门禁不要求迁移历史数据，但明确禁止遗失仍在履行的债务和服务。

## 11. 验收测试矩阵

| 类别 | 必测结果 |
| --- | --- |
| 网关协议 | 正确／错误 HMAC、时间窗、原始 body 字节差异、无签名通知不入账、不同渠道和旧 revision、错误商户、错误 USD 面额、未知 `paid_source`、官方查单超时与非 2xx。 |
| 充值竞态 | 重复／乱序／迟到事件、超过七天迟到付款、单订单通知冷却、回调早于建单响应、创建超时重试、相同和冲突幂等键、取消后到账、查单暂不可用、两个 Webhook 并发都只入账一次。 |
| 钱包并发 | 多个 Provider 同时开通、老服务预算与新单竞争、扣款与余额退款竞争、Turso 不可用、账本重算与缓存一致、零和与非负约束。 |
| Share／Client | 试用不扣费、开通成功一次捕获、授权／Provisioning 失败完全释放、报价过期或商品变价重新确认、健康停服不生成虚假履约。 |
| 固定月租 | 长短月、闰年、到期取消、余额不足 recovery、自动续期优先级、提前终止退未履约款、尾数守恒、同一期间不重复扣款或确认收入。 |
| 收入／提现 | 七天前不可申请、恰好七天边界、月租按履约逐份解锁、争议／退款与解锁并发、提现地址变更、重复申请／审批／已付、重复链上流水号、已付后冲正。 |
| 用户退款 | 只能退可用额、申请冻结、拒绝释放、线下付款引用唯一、用户退款与购买并发、`refund.recorded` 不自动改账。 |
| 安全与产品 | 非 Owner 无财务管理权、Provider 不能看别人的收入、公开 API 不泄露资金、Secret 不在日志／错误／Settings 读响应中、渠道多开隔离、充值回来必重新报价、旧付费 API fail closed。 |

执行门禁至少包含 `cargo fmt --check`、`cargo test`、`cargo clippy --all-targets`、`cd frontend && npm run typecheck && npm run build`，以及现有 `audit:market-billing-contract`、`audit:settings-contract`、`audit:settings-i18n` 与新增资金契约／网关 fixture 测试。真实渠道验收需记录脱敏收据和网关查单事实，不把真实密钥或原始支付 body 加入仓库。

## 12. 非目标与后续扩展

本期不处理旧余额迁移、合规评估、DujiaoPay 出款、Provider 自动提现、多资产钱包、动态 FX、平台佣金或买家从多个资金源拆单支付。以上能力均有明确扩展点，但不能为了预留它们削弱首版同资产零和、来源核验或七天沉淀约束。完成定义是资金可以逐笔对平且所有失败路径可恢复；「能跳到收银台并看到成功页」不构成完成。

## 13. Owner 影子验收与切换清单

1. 先备份 Router 数据库及支付密钥，部署迁移但保持 `disabled`。确认 `/v1/admin/market-finance/readiness` 的 `codeIntegrated`、旧域 blocker、严重对账 case 与渠道 blocker；不可用手改数据库模式替代 Owner API。首次启用后切换批准永久保留，不能再切回 `shadow`／`disabled`；应使用独立的暂停开关处置事故。
2. 在 DujiaoPay 官方商户后台配置 Router 固定 Webhook URL，保存只显示一次的 Webhook 签名密钥。Router 渠道 API 验证、Webhook 验签和真实入账是三个独立证据；`whoami` 通过不等于后两者通过。商户必须实际支持以 USD 两位小数建单和可认证的支付来源。
3. 切到 `shadow`，仅由 Owner 以受控最小 USD 金额完成一次真实支付，保留脱敏订单号、官方查单状态、签名回调时间、链上回执和 journal 对账结果。另验证重复／迟到 Webhook、查单短暂失败和回跳不会重复入账。不能把 fixture、测试通知或浏览器成功页计作真实收款。
4. 在旧路径清结并撤下所有付费旧商品和活动报价，核对旧合同、余额、hold、欠款、退款、争议、待确认充值均为零。保持 `shadow` 且 `readiness.canEnable` 为真时，Owner 才可通过 API 切到 `enabled`；不能从 `disabled` 跳过影子验收。之后 Provider 按钱包规则逐项重新发布付费 Share seat 与 Client Host。
5. 小范围验收充值→报价→冻结→开通→履约→争议／退款、资金不足暂停、七天后提现申请→Owner 线下转账登记。每一步对比钱包、未履约、Provider 待解锁／可提现／提现中与 journal 零和。发现严重不变量或网关身份异常，使用 Owner 暂停开关立即停新钱包单与新提现，必要时关闭渠道停止新充值；继续收取并核对已发生支付的回调，已有合同照常履行。已有真实入账时只能向前修复，不能回滚旧二进制。
