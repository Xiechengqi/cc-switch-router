# Share 用户双口径用量限额：完整落地实施计划

> 状态：**双仓本地实现、定向验证与整体 review 完成，`local_verified / live_pending`；未部署**。规格制定于 2026-10-29，实施核对：2026-10-30。第 1–10 节保留设计目标与验收要求，第 11 节记录实际落点、选择和验证边界；不能把本地实现视为真实流量验收通过。
>
> 范围：普通 Share 授权用户可选择按 Token 数量或按模型 API 标价折算的 USD 等值限制使用。Share Market 已发布车位和活跃合同的计费条款、平台钱包、Provider 收益均不在本次改造范围内。
>
> 依据：[现有只读价目设计](design-share-user-model-usage-and-pricing.md)、[Share 协议](../PROTOCOL.md)、[现行架构](../ARCHITECTURE.md)。只读价目旧文档中的阶段三和 Server 禁止计算 USD 的历史边界已由本方案明确修订；旧“下一版 v7”等时间点描述不是本方案的实施基线。

## 1. 交付目标与产品语义

1. 每个普通授权用户独立选择 `tokens` 或 `usdEquivalent`；未设额度继续表示不限额。已设置的 Token 数额和周期原样有效，升级不能修改既有调用许可。
2. `usdEquivalent` 是 **Share 请求的 API 标价等值用量配额**，以 USD 表示。它不是用户平台钱包余额、实际 Provider 成本、市场租金或应付账款。设置“每天 $10”只控制 Share 使用资格，不扣平台余额。
3. 按最终执行模型、有效服务档位、对应上下文档位和非重叠的 fresh input、output、cache read、cache write 计量。一次可计量的上游 attempt 只归属一个已确认的用户、Share、周期和价目版本；一个逻辑请求若实际产生多个 attempt，各 attempt 均须可追踪并计入该用户。
4. 保留 Share 总 Token 限额、Token 模式的用户数量额度、总并发、单用户并发、授权到期、App 权限、上游真实 quota、市场试用限制等原有约束。适用的限制取交集，界面和错误分别指出触发者。
5. 公开承诺是**请求级软上限**：开始请求时检查已确认消耗及在途风险，达到上限后拒绝新请求；已准入的请求允许结束并可能使累计值超过额度。不能宣传为“按输出 token 精确截断”或“绝不超一分钱”。
6. 同一个 Share 可同时有 Token 用户与 USD 等值用户。Share 的顶层 `tokenLimit` 继续是 Token 数量；本期不隐式转换为金额。若未来要为整个 Share 设置 USD 等值总额，复用本规格的计量核，另立产品与协议变更。
7. 既有免费授权仍按原规则运行。没有 active grant 的免费用户不凭空获得一份美元限额；本功能不改变 free access 身份与授权规则。

### 1.1 词汇与单位

| 名称 | 语义 |
|---|---|
| `tokenLimit` | 现有数量额度，正整数 token；缺省为无限制，`0` 不合法。 |
| `valueQuota.limitUsdMicros` | USD 等值额度，十进制整数**字符串**；`1 USD = 1,000,000 micros`，必须大于零。 |
| `rateMicrosPer1m` | 每百万 token 的微美元单价；允许明确声明的零价，缺价绝不是零价。 |
| `usagePicos` | 内部累计精度，`1 USD = 1,000,000,000,000 picos`；由 `tokens × rateMicrosPer1m` 得到，不逐请求先截成美分或微美元。 |
| `pricingRevision` | Router 发布且 Server 已确认的不可变价目快照散列。 |
| `policyEpoch` | 一次明确生效的用户额度策略世代；仅显式策略变更推进，普通请求不推进配置版本。 |
| `windowId` | Server 确定的额度窗口；自然周期使用 UTC，账号重置周期使用已确认的上游周期身份。 |

前端按用户输入的 USD 十进制字符串换算为 `limitUsdMicros`；不接受指数、负数、空格混入或超过六位小数。金额上界和 `i128` 乘积上界通过同一域校验固定并测试，不能依赖 JavaScript `number` 或 SQLite 浮点。内部 `usagePicos` 若落入 SQLite，使用规范十进制字符串及应用层 checked arithmetic，不能直接对 TEXT 使用 SQLite `SUM` 或按字典序比较。

### 1.2 实际使用体验

- 授权编辑中的“用量限额”提供“Token 数量 / 美元等值”切换；周期仍在同一位置。选择美元时输入例如 `$10.00`，并显示“按模型 API 标价折算，不从平台余额扣款”。
- 查看时展示本期已用、额度、剩余、重置时间和计量状态；模型与分类明细保持折叠，不增加常驻宽表列。数量模式继续显示原 token 信息。
- 有一个可确认的超额请求时，界面可显示 `$10.20 / $10.00 · 已耗尽`。金额精度展示至少能区分小于一美分的用量；极小正数不能显示为可误读的 `$0.00`。
- 切换全站 USD/CNY 显示只影响展示。权威额度、消耗、阈值和价目均为 USD；不能随汇率波动改变使用资格。

## 2. 当前实现与必须保持的边界

- Router `src/models.rs::ShareUserPolicy` 只持有 `tokenLimit`、`tokenPeriod`、并发、到期和 App 范围；`src/store.rs::share_user_limit_status` 计算数量额度并通过 `usageRebase` / `usageQuota` 维持现有有效用量。
- Server `src/domain/sharing/shares.rs::validate_for_invocation` 和 `src/state.rs::validate_and_acquire_share_invocation` 在转发路径做真正的用户与 Share 限制；`domain/usage/store.rs::quota_tokens()` 对四类 token 平权求和。
- Server 已有 `src/repository/server_sqlite` 的 schema v2 和 exact quota 表，但安装可仍处于 SQLite shadow、由旧文件承担权威的状态。它的 `quota_usage_entries` 是 Token/account quota 记录，不是金额账本；本功能必须新增独立表和交易路径，不能把 picoUSD 写进现有 `tokens` 列。
- Router 的 `src/model_price_catalog.rs` 和 `src/model_pricing.rs` 已有派生价目、管理员覆盖、别名、服务层级、长上下文分档和只读估算。但派生价目可替换历史，现有 `price_tokens` 每分类截断到微美元；两者都不能直接作为长期、可审计的执行账本。
- Router 请求明细默认只保留 30 天，清理会删除 `share_request_logs`。数量额度的 Server 权威快照不因此归零；新增金额额度也必须独立于报表明细保留。
- 现行 Share Contract reader 到 v9，普通未协商 descriptor 为 v7，v8/v9 能力显式协商。Server 的产品边界目前写明“只统计 Token，不计算 USD”；实施金额额度需要**明确修订此边界**，允许 Server 做纯额度换算，不引入钱包、市场结算或第三方价格源读取。
- `GET /v1/shares/:share_id/user-limit-status` 和 `/user-usage-breakdown` 目前有意匿名可读。旧接口保持原数据形状；新增用户美元额度、价目快照和计量状态使用独立鉴权接口，不扩大公开面。若未来调整旧接口的公开策略，需单独做产品与兼容性评审。
- 现有 `usageRebase` 是 Token 标量，市场托管 grant 有独立 manager 和试用/付费状态。不能把 Token 重基线当成美元重基线，也不能通过普通编辑修改活跃市场托管 grant。

## 3. 价格与用量的规范算法

### 3.1 请求身份与模型

Server 在认证、授权和现有 Share 准入之后，选定真实 Provider/执行模型，再做金额模式准入，且必须发生在首次向上游发送请求之前。仅使用最终选定且可证明的模型、服务层级和价格档位；客户端自己填的 `requestModel` 不能覆盖有效执行模型。Provider fallback 或模型改写在发送前发生时要重新选价并重做准入；发送后出现的独立可计量 attempt 必须有独立计量身份和风险占用，不能把用户逻辑请求与多个真实 attempt 混算。

Router 价目别名仍采用精确或已声明的前缀映射。金额模式不得使用“模型族相似”的猜测定价。所有会使用的执行模型须经允许列表和价目覆盖率审计；未知模型明确标识为 `price_unavailable`。

### 3.2 互斥 token 分类和长上下文

价格输入固定为 `freshInput`、`output`、`cacheRead`、`cacheWrite5m`、`cacheWrite1h`。Server 已有 OpenAI/Gemini inclusive 与 Anthropic exclusive 输入归一化；扩展 5m/1h 字段时仍须维持“fresh + cache read + cache write = raw input”的互斥口径。`quotaTokens` 继续按现有逻辑计算，不能因为新增细分而双计。

当前长上下文查询用 `input_tokens` 判阈值，而该字段是 fresh input。上线前逐模型核对上游的真实判档输入口径，扩展价目元数据记录 `contextInputBasis` 与阈值比较方向；需要 raw input 的模型使用 fresh + cache read + cache write，不直接把“只算 fresh”复制到金额执行路径。若某模型上下文档位无法确定，只有可证明的保守价格边界才允许继续。

缓存写入精度按以下顺序处理：上游明确给出 5m/1h 明细时精确计价；仅给合并写入且当前价目同时声明两档时按两档中较高单价计量并标注 `conservative_cache_ttl`；能够证明该模型只存在单一写入价时使用该价；没有可用上界时该能力在金额模式下不可用。展示层现有“全部按 5m”仍可作为参考，但不得作为执行额度的默认规则。

服务层级必须使用 Server 已确定的有效 tier。未知 tier、缺少适用价格行、可能低估实际档位的 fallback，在金额模式下返回 `price_unavailable`；现有只读展示可继续按原逻辑标注回退。管理员覆盖价需支持完整 `(serviceTier, contextTier)` 费率矩阵和长上下文元数据，修正当前只写 `standard/base`、将阈值置空的覆盖行为，才能用于金额模式。

### 3.3 金额计算与冻结

每个类别的精确内部贡献为 `tokens × rateMicrosPer1m` picos；五项用 `i128` checked add，负数和溢出拒绝。一个 attempt 以它的固定价目版本、有效模型和最终修订 token 数计算。幂等身份至少绑定 `(installationId, shareId, requestId, attemptId, usageRevision)`；同一身份重试不能重复增加，更高用量修订替换上一修订，差额可以正或负，但不能更换用户、Share、周期或价目版本。若适配器能证明只有一个上游 attempt，`attemptId` 可使用固定规范值，不能靠省略字段将多个 attempt 合并。

校验向量示例：fresh input 1,000 token、费率 $3/百万，output 1,000 token、费率 $15/百万，其余类别为零，则分别为 $0.003 与 $0.015，总计 $0.018，即 `18,000,000,000 picos`。若准入前已用 $0.99，随后一个已准入请求实际消耗 $0.03，窗口显示 $1.02/$1.00，下一请求被拒绝；这正是软上限，不是钱款透支。

Router 派生目录升级、`./git-push.sh` 产生的独立价目同步提交，以及 owner 修改模型单价，都只生成**新的价目修订**。生效前的请求继续按其冻结修订；不能全量重算历史执行消耗。管理员纠错若需要补偿既有额度，走带理由、预览和审计的显式额度调整；不能通过改价目悄悄改历史。

Router 与 Server 应共享一份版本化的计量规范及测试向量，Server 持有经 Router 签名/绑定的价目快照本地执行，不依赖每个请求向 Router 查价；Server 不在构建或运行时从外部项目抓取 LiteLLM。价目传输要有大小/数量上限、内容散列、schema 版本和确认回执。Share descriptor 只绑定 `pricingRevision` 与相应能力，不在每次动态用量变化时重新下发大目录。

### 3.4 非标准与不完整用量

- 明确零价与缺价必须是不同类型。缺模型、缺档位、无法确定 token 分类不能写作 `0 picos`。
- 请求尚未终结、Server 重启导致无法确认用量、原始上游仅返回总 token、估算请求可能漏掉上游失败 attempt 时，状态为 `pending` / `estimated` / `unavailable`，保留证据和本地在途占用，不将其静默释放成免费调用。
- 上游能补报准确用量时，以较高 `usageRevision` 自动修正同一计量记录；不允许旧修订覆盖新修订。长期不能确定且没有合理保守上界时，仅暂停相关金额模式准入并给出明确错误；Token 模式沿用原规则。
- 有 token 数但无法覆盖图片、视频或其他独立计费资源时，金额模式只对已验证的文本/Token 能力开放。其他能力另设明确额度后才能开放，不能让非 token 能力因为零 token 获得无限使用。
- 请求失败、499、504、取消、重试都按**实际已记录的用量**处理，不能按 HTTP 成功码筛选。Health Probe 不计用户额度。无法证明被丢弃上游 attempt 的真实消耗时，不宣称这等于 Provider 的最终账单。

## 4. 限额准入与窗口语义

### 4.1 准入顺序

1. 保留现有身份、Share 可用性、有效 grant、App 范围、过期、Share 总 Token 限额、Token 模式的用户数量额度、上游账号保护及并发准入。
2. 对金额用户取得与当前 Router/installation、Share、策略世代绑定的已确认价目。缺快照、hash 不符、旧版本不支持时返回独立 `quota_unavailable`，不向上游发送。
3. 在模型已确定后选择价目，并在一个 Server 受控临界区中计算当前窗口**有效已用量**和在途风险；达到上限时返回 `value_quota_exhausted`，不向上游发送。
4. 准入检查与请求开始事件必须处在同一受控、立即持久化的原子边界内；写入用户、grant/entitlement、周期、策略世代、价目版本及初始在途状态成功后才开始转发。不能检查后释放锁、再异步落盘；写入失败即拒绝请求。
5. 按终态或更高 usage revision 更新同一记录；同步更新权威累计、释放在途占用，并在金额跨过阈值时阻止下一次准入。Server 崩溃后先恢复未完成记录，不把它们当零。

若执行模型在一次请求里变化，要么在首次上游发送前重新进行价目检查，要么按每个真实上游 attempt 的独立计量记录结算。不得把廉价请求模型与昂贵实际模型混用。所有 Share 入口（直连子域名、统一模型路由、Claude/Codex/Gemini API、必要的 Web/CLI 转发）必须共用同一 Server 准入函数；不能只在 Router 前端检查。

### 4.2 软上限和并发暴露

额度在“新请求准入”时检查，已经放行的流式请求自然结束。`spent >= limit` 一定拒绝；可确定的本次输入最低等值金额已经超过剩余额度时也拒绝。在途记录另有**风险预占**，不是真实扣额：有可由上游请求参数实际强制执行的输出上界时按输入及输出的保守估计预占，最多预占当前剩余额度；没有可信上界时预占全部剩余额度，待请求结算后释放，从而防止更多不确定请求同时进入。中途用量修订要将已确认消耗与剩余风险分开更新，不能双计或提前释放；同一窗口跨 `policyEpoch` 的旧在途请求仍占用当前准入风险。`spent + pendingRisk >= limit` 时暂停新请求准入。金额模式的有效单用户并发数取现有 `parallelLimit` 与 **4** 的较小值；原来不限并发的用户在金额模式下最多同时执行 4 个请求，界面明确展示此规则。该上限不改 Token 模式，且不能推出“最终金额永不超限”：单个已准入请求仍可超额；若以后要求绝对硬上限，另做可证明最大输出成本的严格预占与能力限制。额度错误文案不能归咎于钱包余额，也不能无依据承诺“只会小幅超额”。

### 4.3 周期与归属

- `lifetime`、UTC day/week/calendar month、锚定 sevenDays/thirtyDays 沿用现有周期定义。sevenDays/thirtyDays 是锚定固定窗口，并非滑动窗口；不改变 anchor 或 Server 选定窗口的计算函数。
- `accountReset` 只在已有 Router/Server 能力、有效 `AccountQuotaCycle` 与可证明完整历史下允许金额模式。不同 Provider source/epoch/bucket 不得混合，未知新周期不是“已用 $0”。
- 请求开始时持久化 `windowId`，迟到修订仍归原窗口；不能按完成时间把旧消费挪到新周期。策略变化时在途请求按开始时冻结的策略版本和价目版本结算。
- 本功能启用前若没有该用户完整窗口的金额计量证据，当前窗口不可伪装为“从零开始的完整周期”。自然周期默认从**下一个完整窗口**生效；累计和没有可预测边界的账号周期仅在首次引入金额计量时显式确认 `meteredFrom`，界面写明“自此时间起计量”。此后切换模式、改额度或推进 `policyEpoch` 均不得重设该起点；真正要调整历史已用额度，只能走独立审计操作。已在途旧请求没有可验证价目与用量时，不得在当前窗口启用金额模式。
- 双口径计数在具备完整价格证据后均持续维护：Server 从 shadow 阶段起也为 Token 模式请求记录独立金额计量，但不据此拦截 Token 模式；缺价/缺用量在金额窗口覆盖上留下明确缺口。价目快照与计量能力不能只为当前金额用户下发。窗口消耗按 `(Share, 用户, 真实周期窗口, 最早有效 meteredFrom)` 从带时间戳的原始计量记录聚合，`policyEpoch` 只绑定策略和同步版本，**不能**作为让已用金额归零的新分桶。切回金额模式要复用同一窗口内先前金额消耗及在途风险；切换单位本身不能创造新的免费窗口。修改周期或 anchor 必须从原始记录重算新窗口、预览覆盖率并留下显式审计，证据不全则推迟生效；补录、清除或增加已用额度是单独有权限、预览和审计的操作。

### 4.4 手工校准

现有 `userUsageEdits` / `usageRebase` 仍只处理 Token。金额模式新增独立 `valueUsageEdit`，支持以现有窗口为准的 `set` / `clear`，必须携带 expected grant revision、windowId、服务器已观测水位和幂等 operationId。低于已证明真实消耗的 target 拒绝；明确删除人工偏移与删除已观测用量是两回事。Server 在同一 quota mutation 中验证批量编辑，任何一项失败都不得部分落地。

普通 owner 不能用 `valueUsageEdit` 修改活跃 `routerShareMarket` grant，Router owner 的管理员能力也不得绕过 Server 的历史证据校验。金额基线与 Token 基线互不转换、互不覆盖。

## 5. 协议与能力协商

### 5.1 版本安排

当前 Share reader 上限是 v9。若实施时没有更高版本占用，本功能使用 v10，并在安装 heartbeat 的已绑定 Router URL、installation 和短有效期能力声明中增加 `usdEquivalentQuotaV1`。实施时若其他功能已占用 v10，取下一版本，绝不复用旧版本偷加字段。

旧 Server/Router 能继续使用数量模式。未确认双端能力、价目快照和存储健康时，界面不能提交金额模式。新字段不得塞进普通 v7 或现有 v8/v9 descriptor；两端 `deny_unknown_fields` 约束需要同步更新解析、规范化、签名 body、指纹和版本校验。普通市场 v5 最低版本不因本功能整体抬高。

### 5.2 提议的最小线格式

现有 `tokenLimit` 与 `tokenPeriod` 保持，新增仅在金额模式使用的对象：

```json
{
  "tokenPeriod": "day",
  "valueQuota": {
    "limitUsdMicros": "10000000",
    "tariffPolicyId": "api-equivalent-v1"
  }
}
```

规则：`tokenLimit` 与 `valueQuota` 不得同时存在；二者都缺省为不限制；二者都不能为零。`tokenPeriod` 与现有 anchor 继续只定义一个周期，不复制第二个可能冲突的周期字段。域模型可使用 `None | Tokens | UsdEquivalent` 枚举，Wire 层为旧字段保留兼容。`valueQuota` 不含平台钱包、充值费、市场服务费或用户支付金额。Router 保存 Server 回执里的真实策略，不能从前端请求伪造已生效状态。

v10 的 grant 包装层还需携带 `quotaPolicyEpoch`（每次显式策略变更生成的不可复用、不靠时间戳推断的 opaque ID）和 `valueMeteredFrom`（首次金额计量起点；账号周期还绑定 source/epoch/bucket 身份）。两者随静态策略签名、规范化、ACK 并在 Token↔美元切换后保留；删除再创建 grant 也不能复用旧 epoch。旧 v7–v9 grant 不假装拥有这些字段。Server 动态用量和 Router 报表刷新都不得生成新 epoch；Server ACK 返回实际生效 epoch，后续水位必须与之匹配。

Server 通过拟新增的 `POST /v1/shares/:share_id/value-quota-watermarks` 向 Router 发送动态金额水位，包含 `installationId/shareId/规范化email/policyEpoch/windowId/windowStart/windowEnd/activePricingRevision/usagePricingRevisionsDigest/usageWatermark/effectiveUsagePicos/meteringStatus`。`effectiveUsagePicos` 是当前真实窗口跨策略世代、跨已冻结价目修订聚合后的值；`activePricingRevision` 仅表示新请求将使用的修订，不能冒充整个窗口的单一历史价目。`policyEpoch` 是当前配置绑定与来源校验，不是用量分桶；旧世代在途请求结算时，Server 重算当前完整窗口，并按**当前**世代发布新水位，而非重放旧世代投影。来源身份、旧窗口、低水位重复事件必须被拒绝或幂等确认；同一请求的合法更高修订允许累计值下降。金额水位不提高静态配置修订，不能让设置弹窗因每个请求的动态刷新反复冲突。端点复用现有签名/installation 绑定原则，不引入浏览器可写水位；Router 返回已确认水位，Server 持久化后才停止重试。增加端到端 contract 测试。

5m/1h 使用字段随请求日志合同扩展；Server 只向确认该能力的 Router 发送新字段。旧日志的合并标量无法拆分，保持 `unknownTtl`，不得后台“修复”为虚构 5m/1h。价格快照交换与日志交换分别版本化；读者不能把新价目 capability 当成旧日志字段的能力证明。

### 5.3 价格快照握手

Router 生成规范排序、独立 schema 版本、签名/哈希的不可变费率快照；Server 经拟新增的受认证 `GET /v1/shares/pricing-snapshots/:revision` 读取，验证 Router 身份、installation、时效、大小限制、模型 key 唯一性、完整费率矩阵和数值边界后持久化，并通过拟新增的受认证 `POST /v1/shares/pricing-snapshots/:revision/ack` 幂等确认。Router 只有在 Server ACK 后才允许引用该修订提交金额模式策略。Router 控制面断连时，Server 继续使用本地已 ACK 的快照并执行既有额度，停止接受无法核验的新修订。

价目更新有 `effectiveFrom`，在对应 Server 已 ACK 且达到生效点后仅作用于**新请求**。请求开始时同时冻结 `pricingRevision` 和该模型费率行；旧修订保留到关联请求、修订窗口及审计保留责任结束。回退版本不能删除旧快照；数据恢复时不能无证据重建已确认用量。

## 6. 持久化与一致性

### 6.1 Server 权威计量

在 Server 已有的 `src/repository/server_sqlite` 中以新递增 schema 迁移增加独立金额账本、窗口检查点、在途风险和价目快照表；仅在该安装的 SQLite authority 已 `committed` 且事务、备份/恢复验证通过后，允许金额模式生效。仍处于 SQLite shadow 的安装继续原 Token 模式，不能把金额热路径写入会整体重写的旧 usage 文件，也不能把 picoUSD 塞进现有 `quota_usage_entries.tokens`。所有读改写通过 `ServerStateInner` 域方法封装，遵守现有锁顺序；金额准入检查、风险预占、开始事件必须在同一数据库事务中提交后才能向上游发请求。最少保存 `(installationId, shareId, requestId, attemptId, revision)`、规范化用户、策略世代、请求开始周期、价目修订、模型和档位、分类 token、金额 picos、计量状态及最小审计时间。沿用现有 quota ledger 的 request scope、revision、watermark 思路，但金额账本独立索引、独立完整性检查。

每个活跃金额窗口有可校验的累计检查点和修订水位；Token 模式 shadow 计量也保留覆盖证据，供将来切换口径使用。报表请求明细被清理、Share 脱线、Server 重启或 Router 重启，不得让额度归零。迟到修订需要在尚可修订的窗口里替换旧值；窗口关闭后的证明仍可用于对账但不能重开过期授权。窗口关闭且全部请求已确认终态后，才可压缩到不可归零的累计检查点，并保留足够的去重墓碑供迟到重放识别。`lifetime` 等长期不关闭的窗口也需要可证明的压缩：终态记录可转为保留完整不可变 scope、最高修订、既记金额、模型/档位和冻结价目引用的最小行，并维护核验过的累计检查点；这种最小行仍须足以处理合法迟到修订，不能简单删除。仍 pending 的请求绝不压缩；压缩之后收到无法归属/验真的迟到修订时隔离该窗口并告警，不能猜一个差额入账。为高使用量窗口提供容量测试、索引/归档预算和告警，而不是沿用现有 Token ledger 的固定百万修订上限；若无法证明精确状态，相关金额准入 fail closed，不能通过裁剪/静默删除解决。

手工金额基线记录为单独审计事件，有 actor、理由、before/after、observed watermark、窗口和操作身份。它不是 `share_request_logs` 中的一条伪请求。

### 6.2 Router 投影与访问权限

Router 的 schema 使用**新递增迁移**追加价格快照元数据、金额用量投影和审计，不改冻结的 baseline、不重写历史钱包/市场表。表/JSON 字段只存额度数字、revision、窗口、状态与脱敏原因；不得写入凭据、请求内容、用户平台资金或邮件验证码。投影写入按 Server 水位和身份做条件更新，不能按时间戳猜“新的就是权威”。Router 状态滞后时显示“同步中/上次确认时间”，不把缓存的 `$0` 当当前额度。

新增两类只读 API：`GET /v1/shares/:share_id/value-quota-status` 仅允许 Share owner/Router owner 查看该 Share 授权用户；`GET /v1/me/share-value-quotas?shareId=…` 仅允许登录用户查看自己的额度。编辑前的 `POST /v1/shares/:share_id/value-quota-preview` 仅允许 Share owner/Router owner，向 Server 取得当前窗口、计量覆盖、能力、价目修订及冲突水位，不产生额度或价目写入。响应带 `limitUsdMicros`、`usedPicos`、`windowStartsAt`、`resetsAt`、`meteredFrom`、`activePricingRevision`、`usagePricingRevisionsDigest`、`policyEpoch`、`meteringStatus`、`syncedAt`；状态和时间不能省略后由前端猜成零。三个接口都必须在后端鉴权并返回 `Cache-Control: private, no-store`，前端切账号/登出清掉旧内存响应。现有公开的 `user-limit-status`、`user-usage-breakdown` 不附加新的私人金额额度字段；匿名公开面如需展示，仅使用无邮箱、无合同额度的既有聚合价目信息。其他会序列化 Share descriptor/grant 的 Router API 也需检查并收窄字段，不能只检查上述两个旧端点。

## 7. 改动位置与实施工作包

### WP0：规格和覆盖率门槛

- 更新 `PROTOCOL.md`、`ARCHITECTURE.md`、Router/Server 各自的 usage 口径文档和协议版本审计脚本。
- 从脱敏真实请求样本统计：执行模型识别、四分类 token、有效 tier、长上下文判档、5m/1h TTL、估算和未知请求的覆盖率。记录每个 Provider/App 能否安全启用金额模式；启用范围里的模型及实际可选档位必须 **100% 有执行价格或明确保守上界**。不能凭“LiteLLM 收录了模型”推断生产计量完整。
- 建立跨双仓相同的黄金测试向量与明确的软上限说明；确定完整隐私与错误响应契约。

### WP1：Server 用量增强与 Router 价目校准

- Server `src/domain/usage/store.rs` 补解析并持久化 5m/1h 缓存创建，保持既有 raw/fresh/cached 求和不重叠；日志同步通过协商附带新字段。
- Router `src/model_pricing.rs`、`src/model_price_catalog.rs` 增加执行配额用的高精度纯函数、完整档位覆盖校验、价目快照与修订管理；展示价目仍可按现有逻辑纠错，但执行历史冻结。
- Router `src/store.rs` 两处长上下文分档及相关价目显示按实际模型价目规则校正；Server 使用同一可验证口径。管理员价格 UI 支持完整矩阵与上下文元数据，已存在的简化覆盖价要标记不合格或经 owner 明确补齐后才能进入执行价目。

### WP2：双端协议及持久化

- Router `src/models.rs`、`src/api.rs`、`src/store.rs`、Share edit/ack 路径增加策略、快照、金额投影和鉴权读接口；新增顺序 schema 迁移和不改历史钱包的迁移测试。
- Server `src/domain/sharing/router_contract.rs`、`src/domain/sharing/shares.rs`、`src/state.rs`、`src/repository/server_sqlite` 增加 v10 或后继版本解析、能力协商、递增 SQLite schema、持久化金额 ledger、幂等与修订；状态写入通过 `state.rs` 封装。SQLite shadow/committed 两种安装态都要验证：前者不得启用金额限额，后者重启后不丢占用与用量。
- Server 用同一 `(installationId, shareId, requestId, attemptId)` 身份及单调 `usageRevision` 连接流式更新、重试、迟到修订、启动恢复。Router 的金额水位导入与 Server 回执做身份、policyEpoch/window 和水位校验。

### WP3：执行和错误契约

- 将金额检查放在 Server 模型已选、上游调用未发出的共同转发路径；在 Provider fallback、Web/API/统一入口共用同一个域方法。
- `value_quota_exhausted`、`price_unavailable`、`quota_unavailable`、`quota_revision_conflict` 分别表达额度耗尽、缺价、存储/同步不可用、版本冲突；保留原 Token/上游 quota 的错误含义。不能将金额额度错误说成“平台余额不足”。
- 加入在途风险、落库失败、未完成请求恢复及超过额度时只拒绝新请求的测试。

### WP4：Router 和 Server Web 编辑界面

- Router `frontend/components/dashboard/share-edit/share-user-grants-editor.tsx`、只读表、批量策略逻辑、API/types、i18n 增加单位选择、精确金额解析、预览与状态；金额能力未确认时控件明确提示升级，不允许保存空语义值。沿用现有保存回执，批量项默认不勾选。
- Server `web-src/src/components/providers/ShareUserGrantsEditor.tsx` 及对应类型/翻译实现同一策略含义，不从 Router 复制组件。普通授权与市场托管授权保持分界。
- 审核 `Settings → 模型价格` 的配置入口和逐模型覆盖提示；金额能力启用前必须能定位缺价模型和必要的价目修订。Token 等值展示旁要标明其参考性质，执行额度使用的已冻结金额单独显示。

### WP5：市场边界与运维

- 所有活跃 `routerShareMarket` grant 保持只读及原冻结 `tokenLimit`/试用/租金。市场车位报价、预留款、平台钱包消费和 Provider 收益不因 `usdEquivalent` 自动改变。
- 若后续决定“新发布车位销售美元等值限额”，另扩展 seat/listing、报价快照、managedGrant、续期同意、租客可见条款与回收证据；禁止用普通授权编辑侧门修改已成交合同。本工作包只做旧轨道保持测试和未来扩展钩子，不把市场商品切换视为本次完成条件。
- 运维看板提供启用数量、价目/用量覆盖率、未知用量、待修订请求、同步延迟、失效快照、接近 ledger 容量以及按原因拒绝计数。提醒按 `(Share,user,window,reason)` 去重，避免每个请求发送重复邮件。

## 8. 部署、开关与回退

1. 实施前备份 Router 数据库与 Server SQLite/旧 state/usage；确认当前安装绑定、协议版本、Server SQLite authority、真实模型覆盖率和价目管理健康。金额模式只对已安全切换为 SQLite `committed` 的安装开放；其他安装先按 Server 现有的 shadow→prepared→committed 迁移流程单独验收，不能以启用金额配额代替存储切换验收。备份及恢复过程本身不改旧余额/租金表。
2. 先部署理解新版本**但默认不发送/启用**的 Router reader、价目快照服务与迁移；旧 Server 继续 v7/旧数量模式。随后部署支持能力的 Server，等待已签名 heartbeat、价目 ACK、存储健康和同步水位确认。
3. 只在已证明覆盖的内部 Share/用户上 shadow 计算与现有展示对账；shadow 不拦截、不改变 Token 额度。其开关独立于平台钱包财务 shadow 模式。
4. 小范围显式开启金额模式，并记录 owner、Share、用户、额度、窗口起点、价目修订和能力 ACK。默认从下个完整自然窗口生效；累计模式由 owner 明确确认 `meteredFrom`。逐步扩大到已验证的 App/模型。
5. 开关关闭时停止接受新的金额模式编辑；已有金额用户的 Server 必须继续使用冻结快照执行。应急需停用 Server 金额执行时，先通过同一版本化、可审计的授权操作把受影响用户切回经预览的 Token 策略或暂停其 Share 授权，等待 ACK，不能让旧 Server 忽略金额字段后变成无限制。
6. 回滚 Router 或 Server 到不认识 v10 的二进制前，检查所有金额策略、在途请求、价目快照和金额水位；存在任何 active 金额策略时停止降级部署。恢复使用备份时必须按身份和水位证明不会回退已用额度，不能把历史快照当实时零余额。

## 9. 测试矩阵与验收门槛

### 9.1 单元与契约测试

| 用例 | 必须证明 |
|---|---|
| 同数量不同成本 | 输入、输出、缓存读、缓存写和不同模型各按其费率累计；类别不双计。 |
| 极小请求/边界 | 多个小请求的 pico 余数不会丢失；`spent == limit` 拒绝下一请求；负数、溢出、非法小数拒绝。 |
| 长上下文 | 含大量 cache read 的 raw input 按模型规定判档；阈值两侧、inclusive/exclusive 都正确。 |
| 5m/1h | 明确拆分按两价；只有合并量时按上界并标注；无可信上界时禁止金额准入。 |
| 模型与 tier | 实际模型、别名和有效档位决定价格；未知/改写/缺档位不能回退到廉价价格。 |
| 重试与修订 | 同 ID 同修订幂等；高修订按差额替换且可下降；不同用户/窗口/价目不能共用 ID。 |
| 失败与中断 | 499/504/失败仍计已观测用量；无证据和 Health Probe 不被假定为等额或成功消费。 |
| 周期 | UTC 日/周/月、锚定 7/30 天、账号 source/epoch/bucket、跨窗口在途完成和迟到修订。 |
| 并发 | 已准入请求可使总额软超限；之后的新请求拒绝；在途数有界，不造成无上界并发放量。 |
| 生效与切换 | 旧 Token policy 完全不变；未满窗口启动预览；数量↔美元切换不恢复消耗；手工偏移与观测分离。 |
| 价目发布 | Server ACK 前不能用新价；派生目录或管理员价更新只影响新请求；历史冻结快照可恢复。 |
| 隔离与权限 | Share owner/本人可见各自允许数据，其他用户和匿名者不能获得新增金额额度；旧公开接口无新增敏感字段。 |
| 数据生命周期 | Router 请求明细清理 30 天后累计仍正确；Server 重启/崩溃、Router 断连和恢复不归零。 |
| 市场不变性 | 试用、活跃 managed grant、租金、钱包和 Provider 收益未被金额额度重解释。 |

### 9.2 本地验证和生产验收

- Router：协议/模型价目审计、迁移测试、定点计算与限额状态单测、Share settings/批量编辑逻辑测试、类型检查和构建；现有数量模式全量回归。
- Server：协议版本/能力协商、三种 App 的各转发面、计量分类、持久化恢复、并发和错误封装测试；`cargo fmt --check`、`cargo check`、定向及必要的全套测试；不把未做真实输入验收标成 live verified。
- 双端：新旧版本双向组合测试，含旧 Router＋新 Server、先升级 Router＋旧 Server、完整升级、Router 断连、价目 ACK 丢失、重复动态水位和降级拒绝。
- 前端：纯逻辑单测、i18n key parity、DOM 结构人工验收；本次实施约束为**不截图、不执行 UI 自动化**。若现有 CI 的浏览器夹具需随契约更新，可更新夹具但本地不运行 UI 自动化。
- 生产灰度：每个拟启用的 Provider/App 转发面至少观察 **7 个自然日、100 条真实完成的文本请求**；样本不足的转发面维持 Token 模式。对于新增、低频模型，须有已核对的官方价目/档位证据和该转发面解析器的黄金样本，不能仅凭同一家族模型的流量放行。只选取不含敏感内容的计量摘要，将 Server 额度 ledger 与 Router 投影按 requestId/窗口/价目修订逐条对账。启用范围内的模型、执行 tier、上下文和每笔已完成 token 分类，必须全部可精确计价或走已声明的保守上界；有一笔无从解释的低估、重复累计或水位回退就不能开启该范围。估算和保守计价的比例应如实记录并在 owner 预览可见，不能用总体高覆盖率掩盖某个昂贵模型的缺口。

**完成定义**：满足第 1 节全部产品行为；新旧协议按握手 fail closed；金额在 Server 请求路径生效且重启后不归零；价目更新不改旧消耗；缺价与缺用量不免费放行；两端编辑和用户视图准确；新增金额数据没有进入钱包/市场账本或未授权公开接口；灰度真实计量对账通过。任何一项未满足，都不能宣称金额限额已生产可用。

## 10. 估算与交付拆分

该工作涉及两仓、协议、用量解析、持久化、所有转发面的热路径准入和两套 Web 编辑界面，属于高复杂度。以熟悉两仓代码的工程师估算，约 **6–10 人周工程工作量**，另需至少 7 个自然日的真实流量灰度观察；价目或用量覆盖不足会延长日程。优先顺序为 WP0 → WP1 → WP2 → WP3 → WP4 → WP5；每个工作包均需独立 review 和验收后才开启下一能力。整个功能只在完成第 9 节生产门槛后标记完成，不以 UI 已显示“美元”作为完成依据。

## 11. 实施记录与整体 review（2026-10-30）

### 11.1 实际交付与设计选择

| 工作包 | 实际落点 |
| --- | --- |
| WP0／WP1 | 两仓各自实现纯定点计价模块及同一份黄金向量；完整模型／tier／context 矩阵、raw/fresh 阈值、独立 5m/1h TTL；更新协议与架构文档。 |
| WP2 | Share reader v10，未协商普通 descriptor 仍为 v7、市场最低 v5 不变；Router 迁移 101；Server SQLite schema v3 独立金额 ledger，不改钱包和旧 Token 账证。 |
| WP3 | 在 generic 原生文本真实模型／tier 确定后、首次向上游发请求前做持久准入；IMMEDIATE 事务、全部剩余额度风险预占、最高修订幂等、流式风险保留和迟到修订。仅开放下述三组实际 profile／driver，其他转发面明确拒绝金额模式，不暗中免费放行。 |
| WP4 | Router 与独立 Server Web 的单项／批量单位编辑、权威预览后再次保存确认、私人用量状态、剩余额度和耗尽提示；金额校准独立审计；Router Owner 配置执行价目与逐模型／档位覆盖检查。 |
| WP5 | 市场托管授权保持原条款；Router 私人投影与健康概览、Server ledger／gap／容量诊断、按原因拒绝指标；不逐请求发送邮件。真实 Provider/App 计量覆盖率仍待灰度采集。 |

执行价目入口位于 Router「设置 → 模型价格 → 执行额度价目」。这是**独立、默认空白、由 Owner 核对后发布的执行快照**；LiteLLM 派生参考价目、只写 standard/base 的旧覆盖价不会自动变成权威额度价目。JSON 编辑器允许填写完整矩阵和上下文元数据，辅助检查精确模型 ID、缺行、缺价和缓存策略；缺价不自动填零。发布后由 Server 拉取、校验、落盘及 ACK，新请求才使用新修订。

实现选择比参考目录更严格的模型识别：执行快照只使用精确模型 ID 和声明的 `exactAliases`，不复用参考目录的前缀／同族猜测。上游报告的日期版本等名称也须逐项核对并声明别名，不能自动当成同价模型。

首期运行时白名单为 `claude.anthropic_api_key` + `http.anthropic_messages`、`codex.openai_api_key` + `http.openai_responses`、`gemini.google_api_key` + `http.gemini_native`，对应 Messages、Responses、Chat Completions、Gemini 文本请求。Web Session／Coding Plan 可复用原生 ProviderType，因此必须核对实际 profile／driver；其他自定义 profile 不自动继承支持。客户端自定义 function tool 的文本允许，Server 不执行工具；内置付费工具和媒体不在本期开放范围。

第 5.3 节原拟 GET snapshot 接口实际选用 installation Ed25519 签名 **POST** `/v1/shares/value-pricing/fetch` 和 `/v1/shares/value-pricing/ack`，以复用现行 action／时间戳／nonce／installation 绑定。动态水位保持 POST `/v1/shares/:share_id/value-quota-watermarks`。私人状态、预览与校准路径详见 [PROTOCOL.md §13](../PROTOCOL.md)。缓存 TTL 日志通过独立 `cacheCreationTtlV1` 协商，不能从 USD capability 推断。

首次完整自然窗口不足时，Server 持久保存 `pendingValueQuota`，旧 policy 继续执行；新增授权在等待期间不开放。已有完整 shadow 证据才可立即生效。lifetime 首次从确认的计量起点开始；存在旧未覆盖在途请求则拒绝立即启用。删除再添加授权复用最早持久 coverage，切换模式／改额度／换 epoch 都不能清零。切换周期须核验新真实窗口，不能复用旧窗口零值。

### 11.2 整体 review 补强

- 预览与保存使用相同 coverage 判定；预览不写 coverage、不生成 epoch。排期回执按 Server 返回的 pending policy 核对，不能误报保存失败或伪装立即生效。
- 单个 USD／shadow 状态异常与普通 Token 用户隔离；金额结算异常通过 savepoint 回滚金额变更、保留旧风险，真实 Usage／Token 证据与 USD-only 数据库故障标记一并持久化，再写独立外部故障标记。
- 价目失效、未覆盖文本面、媒体、内置付费工具、多 attempt／retry 均 fail closed。旧 Router 无法接收活跃美元策略时仅隔离受影响 Share；已撤销授权可以同步撤销投影，历史本地策略和金额证据仍保留。
- 保存预览及转发早期都核对真实 profile／driver，防止兼容 ProviderType 将 Web Session／Coding Plan 导向特殊 forwarder 后绕过计量；免费 Share 的个人授权撤销后不继续强加旧金额策略，私有 Share 则在发送前再次拒绝撤销竞态。
- 原生 Gemini 从真实请求 URL 取得执行模型，不向 JSON 人为注入 `model`、也不信任冲突的 body 字段。原生 API 的响应模型证据与用量在同一事务进入账本；流式证据跨分片累计，冲突保持到终态。仅接受执行模型本身或冻结快照中明确声明的 exact alias，不凭同族名称或日期后缀猜测价格；证据不符保留在途风险并阻止后续美元请求。
- Gemini 金额模式只允许 `generateContent`／`streamGenerateContent`；`countTokens`、embedding、batch 等未覆盖动作在发网和金额预占之前拒绝，避免辅助接口没有生成用量却永久占用额度。普通 Token 模式不受影响。
- 没有模型字段的后续用量帧保留已有模型及来源，不能抹去 WebSocket→HTTP fallback 已记录的证据；跨帧矛盾保持 sticky。此项是旧轨道兼容性修复，不表示 WebSocket／OAuth 已获准使用金额额度。
- 删除授权或候选批量保存失败不能消除持久 coverage；此后未计价请求仍留下 gap，重新添加不能误认作完整零用量。两端 UI 区分整体计量能力与单用户窗口预览，避免一个用户历史不完整导致其他用户无法选择单位。
- Router ACK 高于本地真实水位时冻结对应 scope；托管恢复前写独立 restore fence，不允许备份覆盖 fence 或 degraded marker。**当前不提供一键清除标记／伪造对账恢复功能**。
- 精确 UTC 日检查点、校验和和索引记录数核对；全部状态查询处于同一个 SQLite 读事务，避免并发修订造成伪损坏。终态 30 天后有界压缩，保留身份、最高修订、金额、digest 和冻结价目引用，pending 永不压缩。
- 私人响应按 owner／本人鉴权，均 private/no-store；匿名旧用量接口不扩充金额数据。切账号取消旧响应，不接受旧 epoch、旧窗口或低水位覆盖新投影，Router 记录自己收到水位的时间而非信任 Server 时钟。

### 11.3 本地验证与未完成的生产门槛

本次不截图、不运行 UI 自动化、不修改远程环境，未创建 commit 或推送。其他会话的无关改动及运行进程保留。验证结果如下：

| 范围 | 实际结果 |
| --- | --- |
| Router Rust | 全量初跑 1,683 项：1,682 通过，唯一失败为旧测试仍期待 `expected 2..=9`。实现已正确支持 v10、拒绝 v11；将测试改为按协议常量核验并覆盖 v6–v10 后，定向复测 1/1 通过。这次修正只改测试，之后未再次运行整套约 26 分钟的回归，不能把首次命令记作 exit 0。另迁移 73 项、Share 金额配额 12 项、Share 用户授权 9 项均通过。 |
| Server Rust | 最新严格 Clippy 全 targets 通过；全量 3,936 项单元测试和 132 项集成测试通过，0 失败、默认 3 项 ignored。10 万条容量夹具另行显式通过；其余两项环境驱动测试未运行。 |
| 本地转发夹具 | 四种入口 × 流式／非流式 × 正常／异常上游模型，共 16 组通过。证明发网前已有 durable admission、正常精确结算 `$0.018`、允许单次软超限但拒绝下一请求、模型异常保留风险。缺价、未覆盖 profile／driver 和 Gemini 辅助动作不发网；辅助动作不产生美元占用。无真实凭据或外部上游。 |
| 前端纯逻辑和构建 | Router 额度／价目 8 项、Share settings 13 项；Server 额度／批量策略／i18n 29 项通过。两端类型检查、生产前端构建通过，不含浏览器／UI 自动化。 |
| 审计与 smoke | Server Node 审计 1,911 项、纯 smoke 工具测试 13 项；Router 版本／CI 11 项通过。设置／i18n／模型价格／Token 单位／钱包边界、Server 产品／Web／SQLite 权威边界、生成契约、文档索引、格式及空白检查通过。Server 本地 HTTP smoke 的健康、登录、Provider／Share 创建通过。 |
| 容量证据 | 100,000 条最小终态 attempt 的 lifetime 累计精确、作用域索引命中，本机单次读取约 52 ms。仅为精确性／索引夹具，**不是并发负载、生产吞吐或 SLO 验收**。 |
| 离线 readiness | `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` 返回 `failures=0`、`decision=blocked`：聚合本地测试显式跳过，真实流量与部署输入缺失。不能把这个 blocked 写成 release ready 或 live verified。 |

全量 review 中发现并修正了 Gemini native passthrough 模型识别、旧 body 映射误标真实 URL 模型、上游模型证据与用量分事务产生的短暂放行窗口、空用量帧清除 WebSocket fallback 证据，以及 Gemini 辅助动作误占额度。兼容性版号复测和金额专项已加入 Router CI 的既有编译复用 job，不增加第二次全量编译。已有旧编译 dead-code warning 与 Web 大 chunk 提示不是新增失败，也未通过屏蔽 warning 来绕过检查。

仍须由部署后的灰度独立完成第 9.2 节要求：每个准备开放的 Provider/App 转发面 **7 个自然日、至少 100 条真实完成文本请求**，逐窗口／请求／价目对账。OAuth、媒体、内置付费工具、未知用量和多 attempt 不在本期安全开放范围；不能因 UI 可选择美元额度而擅自开启。离线手工替换权威数据库不在托管恢复 fence 保证范围，禁止在线手工替库。当前仅在本地完成实现与回归时，应标记 `local_verified / live_pending`，不能标记生产 `live_verified`。
