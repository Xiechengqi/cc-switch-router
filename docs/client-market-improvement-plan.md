# Client Market 改进实施计划

本计划写于 2026-11-26，同日已按本文实施（实施状态见文末「实施状态」）。它承接 [在线 Client 租用规划](client-market-online-client-leasing-plan.md)，其中与本计划冲突的条目（D11、§3.2、§8「卖家租期内不能下架」、§5.6 中修改价格需下架重挂）在实施时一并改写。代码位置以撰写时的 `master`（`1eb8d8d`）为准，迁移编号以实际合入顺序为准。

## 0. 已确认的决定

| 编号 | 决定 |
|---|---|
| C1 | Client Market 卖的是「带高质量出口 IP 的 Client 服务」。出口 IP 是卡片的核心信息，硬件规格降为次要信息。 |
| C2 | 平台不做 IP 质量打分，也不接第三方 IP 数据库。公开列表只展示 IP 前三段（IPv6 为 /64）和出口国家。用户自己用在线 IP 检测网站评估。卡片上**不展示**「IP 变更次数」「是否独享该 IP」。 |
| C3 | 租户邮箱不公开，只有买家本人、卖家和 Router 管理员可见。卖家邮箱保持公开。 |
| C4 | 卖家可以收回已出租的 Client：默认提前 24 小时通知；租户违规时可立即收回。只退还未使用的租金，不额外赔偿。 |
| C5 | Share 的买家规则同步到 Client：仅试用、精简结账、购买后可改「试用后继续/结束」。仅试用并发上限两个市场分别计算，各 3 个。 |
| C6 | 供应商排名本轮不改。商店初版排序只按**累计接入时长**（卡片上的「运行时长」，从首次接入 Router 起算），越长越靠前。 |
| C7 | 卡片在线率从 24 小时改为 **30 天**。 |
| C8 | 卖家可以给挂售设置公开标题，最多 30 个字。 |
| C9 | 产品名保持「Client 市场」。界面统一称「Client」，人称统一为「卖家/租户」，去掉「客户端/主机/设备」的混用。 |

为让 C2 真正可行，当前租户（包括仅试用）需要看到完整出口 IP（§6.2）。目前完整 IP 只对卖家可见（`visible_host_ip(.., owned)`，`src/online_client_market.rs:1098`），租户也没有终端可以自测。

以下默认做法没有单独询问，如需调整请在实施前指出：
- 修改价格和试用只影响新订单，已成交的租约按原合同条款履行和续费，与 Share 一致。
- 「我租的 Client」放在 Client 市场页顶部，账户页的租用记录保留。
- 界面沿用现有 Router 视觉体系（`globals.css`、`tokens.css`），组件对齐 Share Market，不另起新风格。

## 1. 阶段与顺序

| 阶段 | 内容 | 规模 | 迁移 |
|---|---|---|---|
| P0 | 前端拆分：`online-market.tsx` 按职责拆开，行为不变 | 小 | 无 |
| P1 | 租户邮箱不公开 | 小 | 无 |
| P2 | 卖家收回 | 中 | 有 |
| P3 | 卖家挂售管理：公开标题、修改条款、暂停接单、历史记录 | 中 | 有 |
| P4 | 仅试用与精简结账 | 中 | 有 |
| P5 | 商店：出口为中心的卡片、30 天在线率、按接入时长排序、筛选/搜索/分页、详情抽屉；租户可见完整 IP；租期内出口变化提醒 | 中 | 无 |
| P6 | 买家：「我租的 Client」、开通引导、租用详情精简、计费小时展示、风险说明 | 中 | 无 |
| P7 | 收尾：用词统一、组件统一、旧兼容代码清理（有前提条件） | 小到中 | 视情况 |

- 所有阶段都只改 Router，Server 不需要升级。
- P0 先做，可以减少后面各阶段在同一个大文件上的冲突。
- P1 是正在发生的隐私泄露，排在功能之前。
- P2、P3、P4 都会改挂售或结账的事务，分开合入，便于排查问题。
- P5、P6 主要是前端改动，可以并行。

本轮不做：IP 评分、第三方 IP 数据、运营商展示、AI 服务可用性自动检测、Provider Rank 调整、收回次数计分、多租户共享一台 Client。

## 2. P0 前端拆分（行为不变）

`frontend/components/dashboard/client-market/online-market.tsx`（983 行）把商店、卖家视图、结账、下架都放在一个组件里。拆分为：

| 新文件 | 职责 |
|---|---|
| `client-market-workspace.tsx` | 分区切换、数据加载与轮询、URL 参数 |
| `shop-catalog.tsx` | 地区筛选、卡片网格（P5 扩展） |
| `client-checkout.tsx` | 报价、提交、回执恢复、原页充值（P4 改造） |
| `seller-workspace.tsx` | 挂售中、已租出、历史，以及下架弹窗（P2、P3 扩展） |

商店卡片 `ClientOfferCard` 和卖家卡片 `OnlineClientCard`（`online-shop.tsx`）合并为一个基础卡片加三种用法（商店、卖家、租户），统一排版密度。拆分只搬代码，不改文案和行为。现有的前端单测和浏览器回归脚本（`test:client-checkout-browser` 等）作为验证依据。

## 3. P1 租户邮箱不公开

### 3.1 现状

- `GET /v1/dashboard` 不登录也能访问（`src/api.rs:1905-1956`，只有带凭据时才要求会话）。
- `src/store.rs:12867-12891` 为每份未释放的租约生成 `DashboardClientRental`，其中 `tenant_email` 不判断查看者是谁。前端在 Clients 卡片（`client-board.tsx:523`）和抽屉（`:1468`）中显示。
- 间接泄露：租户在租用的 Client 上创建的 Share，归属邮箱是租户邮箱（`src/online_client_market/shares.rs:39-43`）。Dashboard 的 Share 列表公开 owner 邮箱（`src/store.rs:13109`），而且挂在物理 Client 下面。

### 3.2 改动

- **后端**
  - `DashboardClientRental.tenant_email` 改为 `Option<String>`（`src/models.rs:4143-4154`）。查询额外取 `buyer_id`、`seller_id`，只有查看者是买家、卖家，或 `viewer_is_admin` 为真时才返回邮箱。
  - Dashboard 中绑定到租约的 Share，owner 邮箱按同一规则用 `mask_email`（`src/store.rs:29012`）打码。
  - 确认租约开始和结束时 Dashboard 缓存的 `generation` 会递增，避免旧响应残留。
- **前端**
  - `frontend/lib/types.ts:211` 的 `tenantEmail` 改为可空。
  - `client-board.tsx` 两处：有邮箱时显示「租户 xxx」，否则显示「已租出」或「自租」。
- **不改**：卖家库存、退租记录、买家租用记录、管理员审计本来就只给当事人看。租户主动把 Share 挂到 Share Market 时，以 Share 卖家身份展示，属于租户自己的选择，也不改。
- **文档**：改写 D11、§3.2，以及原规划 §6 验收步骤 4 中「匿名查看的完整邮箱」。

### 3.3 验收

- 匿名访客和无关的登录用户：Dashboard 响应中没有租户邮箱，租户建的 Share 的 owner 邮箱已打码。
- 买家、卖家、管理员可以看到完整邮箱。租约结束后，任何人都看不到这份租约。
- 更新 `src/store.rs` 中读取租约字段的既有测试（约 `:53643` 起）。

## 4. P2 卖家收回

### 4.1 规则

| 项目 | 规则 |
|---|---|
| 谁能发起 | 该租约的卖家。自租不适用。 |
| 默认方式 | 提前 24 小时通知。如果已付租期在此之前结束，收回时间提前到租期结束，并关闭自动续费。未开通（`starting`）的订单没有工作区需要交接，选择通知方式也立即结束并全额退回，弹窗显示为「取消这笔订单」。 |
| 立即收回 | 只能选「租户违规」作为原因，并且必须填写说明。记入管理员审计，买家可以在退租记录中发起讨论。 |
| 通知期内 | 租户照常使用、照常计费，自动续费立即关闭（期间租户不能重新打开）；试用中选了「试用后继续」的改为仅试用，避免通知期内开始扣首月，收回时间也不晚于试用结束。卖家可以取消收回，取消后恢复租户原来的自动续费和试用后选择。买家在此期间自行退租，不扣提前退租赔付，并与到期收回一样按卖家责任记录。 |
| 钱 | 按卖家责任结算：收回时刻之后未使用的租金全额退回，不额外赔偿，卖家也不得这部分收入。使用现有 `settle_tx` 的 `responsibility="provider"` 分支，扣留和赔付均为 0（`src/client_market_termination.rs:780-875`）。 |
| 试用 | 已消耗的试用时间不恢复，与现有规则「退租原因不影响试用资格」一致。 |
| 可选项 | 「以后拒绝这个租户」：复用准入拒绝，免费和付费两种 `client_host` 都写入拒绝；在收回真正生效时写入（立即收回、通知期满，或租户在通知期内自行退租），卖家取消收回则不写入。「收回后下架」：默认勾选。 |
| 说明 | 卖家写的说明（最多 2000 字）在收回前后都显示给租户，并写入退租记录，作为讨论的依据。 |
| 排名 | 本轮不计入 Provider Rank，只保留审计记录。 |

### 4.2 数据

新迁移给 `online_client_leases` 增加以下字段：

| 字段 | 内容 |
|---|---|
| `reclaim_requested_at`、`reclaim_effective_at` | 发起时间和生效时间（Unix 秒） |
| `reclaim_mode` | `notice` / `immediate` |
| `reclaim_reason` | `need_machine` / `tenant_violation` / `other` |
| `reclaim_note` | 说明 |
| `reclaim_deny_buyer`、`reclaim_unlist_after` | 两个可选项的开关 |
| `reclaim_cancelled_at` | 取消时间 |

同时加 CHECK 约束：只有 `starting/active` 状态的租约可以写入收回时间；`immediate` 必须带 `tenant_violation` 和非空说明。

租约事件新增三种：`reclaim_scheduled`、`reclaim_cancelled`、`reclaim_executed`。

### 4.3 后端

接口都按 `seller_id` 过滤：

**`POST /v1/client-market/online/leases/:id/reclaim`**，参数为 `{mode, reason, note, denyFutureAccess, unlistAfter}`。处理顺序：
1. 在 Immediate 事务内确认还没有退租记录。`accept_tx` 是先写入者为准，所以买家先退租时，卖家的收回返回冲突。
2. 租约处于 `starting`（尚未开通）时，直接按卖家责任结束，买家的冻结款全额解冻。
3. `notice`：写入收回字段，把合同续费改为手动，并通知买家。
4. `immediate`：先执行 `accept_tx(&tx, lease, Some(seller), "provider_reclaim", None, at)`，再执行 `reconcile::fence`，与买家 `end` 走同一链路（`:985-1030`）。
5. 选了「以后拒绝」时，执行 `market_access::set_product_access_decision_tx(PRODUCT_CLIENT_HOST, DENY)`。

**`POST /v1/client-market/online/leases/:id/reclaim/cancel`**：仅在 `notice` 模式且尚未到期时有效。

**其他改动：**
- `reconcile::cycle`（`src/online_client_market/reconcile.rs:666`）：到期且未取消的收回，按 immediate 的方式执行。之后的撤权、子车位结算、清理回执、结算都沿用现有流程。
- `release`（`:271-298`）：`reclaim_unlist_after=1` 时把挂售置为下架，而不是重新可租。
- 买家 `end`：如果处在收回通知期内，退租责任固定为卖家，不扣赔付。
- 通知：`accept_tx` 对非自愿结束不发通知（`:413-416`）。需要新增 `client_lease_reclaim_scheduled`（通知期开始时）和 `client_lease_reclaimed`（执行时）两类通知，中英文案都要加。退租记录还要为 `provider_reclaim` 补一个显示文案。

### 4.4 前端

**卖家**（`seller-workspace.tsx` 的「已租出」卡片，以及账户页卖家库存）：
- 卡片上增加「收回」按钮，点开弹窗：
  - 收回方式：默认「24 小时后收回」，另一个选项是「立即收回（仅限租户违规）」。
  - 原因和说明。
  - 两个勾选项：「以后拒绝这个买家」「收回后下架」。
  - 一句说明：「买家未使用的租金将全额退回，你不会获得这部分收入。」
- 通知期内，卡片显示「将于 {时间} 收回」和「取消收回」。

**买家**（「我租的 Client」卡片和租用详情）：
- 通知期内显示横幅：「卖家将于 {时间} 收回此 Client，未使用的租金会退回你的余额，请在此之前导出工作区备份。」旁边放「导出备份」按钮。
- 被立即收回时显示收回原因，并提供「查看与讨论」入口。

### 4.5 验收

- 金额：付费月租的退款等于收回时刻之后未使用的部分。
- 通知期：自动续费已关闭；买家这期间退租不扣赔付；卖家取消收回后，原来开着的自动续费恢复（2026-11-27 调整：租户没有选择关闭，不应因卖家改主意被断租）。
- 立即收回：不填说明会被拒绝；执行后租户入口立即失效；买家收到通知。
- 同时操作：收回和买家退租同时提交，只有一个成功。
- 有第三方 Share 车位时：先结算车位，再清理工作区。
- Client 离线时收回：Router 侧照样撤销入口；离线 24 小时后，按既有规则先退款给买家。
- 两个勾选项：「以后拒绝」后，该买家再次报价会被拒绝；「收回后下架」后，清理完成时挂售为下架状态。

## 5. P3 卖家挂售管理

### 5.1 公开标题

- `online_client_listings` 新增 `title`，可以为空。规则：
  - 去掉首尾空白后为 1–30 个字符；
  - 不允许换行和控制字符；
  - 不允许包含网址和邮箱，避免变成广告或私下联系方式。
- 上架弹窗和修改条款弹窗都可以填写，租期内也能改。改标题不提升报价版本。
- 没有填写时，继续显示 `client-xxxxxxxx`。
- 标题在商店卡片、详情抽屉、卖家卡片和租户的租用卡片中显示。

### 5.2 修改条款（不再需要下架重挂）

- `PATCH /v1/client-market/online/installations/:id/listing`，可以修改月租或免费、免费天数、试用天数、标题。挂售中或出租中都允许修改。
- 修改月租、免费天数或试用天数时，提升 `revision`。未提交的报价在提交时因版本不一致返回 409，触发重新报价（现有机制）。
- 已成交租约按原合同条款履行和续费；修改只影响之后的新订单，包括当前租约结束后的下一位租户。
- 从免费改为收费，需要满足收费资格（复用上架预检中的收费资格检查，`paid_listing_blocker`）。
- 前端：卖家卡片增加「修改条款」，弹窗复用上架弹窗（`client-listing-add-dialog.tsx`）的出租方式区块。

### 5.3 暂停接单

- `online_client_listings` 新增 `paused_at`。接口为 `POST .../listing/pause` 和 `POST .../listing/resume`，任何时候都可以操作，不影响当前租约。
- 新迁移更新供给视图（`schema/0130_online_client_supply_availability.sql`）：暂停时 `availability='paused'`。商店、报价、成交、地区统计、准入申请都读这个视图，因此会一致地拦截新订单。
- 商店默认隐藏暂停的挂售；关闭「仅可租」后显示，并标注「暂停接单」。
- 暂停时递增 `revision`，让暂停前取得的报价失效，与 Share 的停止挂售一致。

### 5.4 历史记录

卖家视图底部现在是没有样式的折叠列表（`online-market.tsx:648-668`），改为与账户页一致的 `ExpandableRecordTable`。每行显示：租户（卖家可见）、起止时间、收入、结束原因（含卖家收回）、退租记录入口。收入与管理员审计同源：租金扣平台费后的净收入（`market_provider_earning_lots`）加退租赔付。

### 5.5 验收

- 修改月租后，旧报价提交返回 409；新报价是新价格；当前租约的续费仍按原价。
- 暂停后商店不再显示，直接报价也被拒绝；恢复后可以正常租用。
- 标题的长度、网址、邮箱、换行都被拦截。标题为空时显示默认名称。

## 6. P4 仅试用与精简结账

### 6.1 规则

参照 [市场试用规则 §Share 仅试用](market-trial-rules.md)，与 Share 一致的部分：
- 付费挂售且买家还有剩余试用时，结账默认选「仅试用」。
- 仅试用不检查也不冻结余额，不能开启自动续费。试用结束后自动退租、不扣费，并通知「试用已结束，未扣费」。
- 每个买家最多同时进行 3 个 Client 仅试用，与 Share 分开计算。
- 试用期间可以改为「试用后继续」：此时检查首期余额，不足就在原页充值。改回仅试用时，自动续费随之关闭。试用结束后不能再改。

Client 保留自己的规则：试用按小时计（1–720 小时，迁移 134 前按天）；租用别人的 Client 时，仍需确认「卖家可以读取这台 Client 所在机器的数据」；自租和免费挂售不受影响。

### 6.2 后端

1. **迁移**：重建 `trg_platform_contract_after_trial_kind` 和 `trg_platform_contract_after_trial_update`（`schema/0122_share_trial_only_seat_termination.sql:30-39`），把 `product_kind != 'share'` 放宽为 `NOT IN ('share','client_host')`。
2. **合同准备**：`prepare_contract_tx`（`src/market_platform_contracts.rs:765-777`）允许 `client_host` 使用 `trial_only`。
3. **并发上限**：`require_trial_only_capacity_tx`（`:250-272`）增加 `product_kind` 参数，按产品分别计数。Client 新增错误码 `CLIENT_TRIAL_ONLY_LIMIT` 和 `CLIENT_TRIAL_ONLY_UNAVAILABLE`。
4. **下单参数**：`online_client_market.rs` 的 `Confirm`（`:856`）增加 `afterTrial: "continue" | "end"`，默认 `continue`，兼容旧前端。`commit` 在 `:929` 传入 `trial_only`。选仅试用时：跳过余额要求，拒绝 `autoRenew=true`，并校验剩余试用大于 0。
5. **授权截止时间**：仅试用改为 `now + trial`，替换 `:940` 处的 `trial.max(86400)`。
6. **试用中切换**：`set_after_trial_tx`（`:284-360`）允许 `client_host`。Client 没有 Share 的试用 phase 表，改为判断合同状态为 `pending_activation/trialing` 且 `paid_started_at IS NULL`。
7. **试用结束**：退租原因记为 `trial_completed`（现在是 `workspace_expired`）。同时让 `terminate_after_cleanup_tx`（`:2062-2085`）对 Client 也发出「试用已结束，未扣费」通知；目前在线租约总有退租记录，这条通知会被压掉。
8. **文档**：`docs/market-trial-rules.md` 增加「Client 仅试用」一节；`docs/market-client-trials.md` 补充说明仅试用不恢复试用资格。

### 6.3 前端

**`client-checkout.tsx`**，结构对齐 `share-seat-checkout.tsx`：
- 顶部：价格和本次应付金额。
- 二选一：「仅试用 {N} 天，到期自动结束」（有试用时默认选中）/「试用后继续，按月 $X」。
- 时间线：今天开始 → {日期} 试用结束 → 自动结束或开始扣费。
- 只有选「继续」时，才显示自动续费开关、余额和「补足差额」。
- 数据读取确认始终显示，放在主按钮上方。
- 「计费详情」默认折叠，内容为：余额明细、提前退租赔付比例，以及一句「Client 不可用的小时不收费」。
- 只保留一个主按钮：「开始试用」/「确认租用」/「确认免费租用」。

**租用详情**：增加「试用后：自动结束 / 继续租用」及切换入口。Client 使用自己的弹窗（`client-after-trial-dialog.tsx`），与 Share 调用同一个合同接口。（2026-11-27 调整：两个弹窗真正相同的只有原页充值一段，数据和文案都不同，不抽共享组件，避免改动已稳定的 Share 弹窗。）

**错误码**：新错误码接入 `onlineMarketError`。

### 6.4 验收

- 余额为 0 时可以开始仅试用。到期后自动退租、不扣费，并收到通知；清理完成后 Client 恢复可租。
- 第 4 个 Client 仅试用被拒绝，Share 的仅试用不受影响。
- 试用期改为继续：余额不足时先原页充值再成功；试用结束时正常扣首期。改回仅试用后，自动续费被关闭。
- 旧前端不带 `afterTrial` 时行为不变。报价过期、409 后重新报价、回执丢失后重取，这几条路径仍然正常。

## 7. P5 商店

### 7.1 卡片：以出口为中心

`ClientOfferCard` 改为：

| 区域 | 内容 |
|---|---|
| 标题行 | 卖家标题（没有则显示 `client-xxxxxxxx`） |
| 出口行（最醒目） | 国旗、国家名、IP 网段（如 `203.0.113.xx`）。取消现在的「网络详情」折叠（`online-shop.tsx:270-277`）。 |
| 可靠性 | 30 天在线率、运行时长 |
| 卖家 | 卖家邮箱，点击进入供应商排名详情（复用 Share 目录的卖家展示） |
| 规格（弱化） | CPU、内存、架构 |
| 底部 | 价格、试用或免费天数、「租用」按钮 |

卡片底部一句提示：「租用（含试用）后可查看完整 IP，可用任意 IP 检测网站自行评估。」不点名具体网站。

### 7.2 30 天在线率

- 数据来源：`installation_online_days` 按天保存在线分钟数和观测分钟数，保留 400 天（`src/store.rs:130`）。30 天在线率 = 最近 30 个 UTC 日在线分钟之和 ÷ 观测分钟之和。
- 列表接口新增 `onlineRate30d` 和 `observedMinutes30d`，保留 24 小时的字段以兼容旧页面。
- 观测不足 3 天时，在数值旁注明「观测 {N} 天」。没有观测时显示「暂无观测」。
- 选购和上架弹窗统一使用 30 天。退租和结算仍按 5 分钟样本，口径不变。

### 7.3 排序、筛选、分页

- **排序**：固定一种，可租的排在前面，然后按累计接入时长从长到短（`registeredAt` 越早越靠前），时长相同时按 30 天在线率。本轮不提供排序切换。
- **筛选**：把「大洲 → 国家」两级改成一行国家标签，带可租数量，标签较多时可横向滚动；加「仅可租」开关和搜索框（匹配标题、卖家邮箱、国家名）。
- **分页**：超过一页时复用 `share-market/market-pagination.tsx`。
- 选择的国家和搜索词写入 URL，便于分享，沿用现有 `country` 参数。

### 7.4 详情抽屉

点击卡片打开右侧抽屉，交互与 Share 目录一致（`buyer-catalog.tsx` 的 `Drawer`）。内容：
- 标题、卖家、出口国家与网段、30 天在线率、运行时长；
- 在线日历（复用 `ClientOnlineHeatmap`）；
- 规格；
- 条款：价格、试用、免费天数、提前退租赔付比例；
- 「租用」按钮。

### 7.5 租户可见完整 IP

- `GET /v1/client-market/online/leases`（`:1187`）和个人租用详情（`src/client_market_audit/personal.rs:192-212`）：查看者是该租约的买家且租约为 `active` 时，返回完整 `egressIp` 和 `country`。租约结束后不再返回。
- 租户界面显示完整出口 IP 和复制按钮。`egressDiffers` 为真时附一句「以出口 IP 为准」。
- 上架确认文字中补充一句：租户（包括试用）可以看到完整出口 IP。

### 7.6 租期内出口变化提醒

- 现状：出口只在上架和预检时检查（`probe_listing`，`src/online_client_market.rs:359-372`）。
- 改动：`reconcile` 对挂售中和出租中的 Client 约每小时发一次 `/_ctl/market/check`，带新的 challenge，可与每小时的硬件刷新合并。
  - Server 的 check 只做出站回调和读取硬件（`cc-switch-server/src/api/market_workspace.rs:199-232`）。实施时还要确认它在租期内可以调用。
- 出口变化时：
  - 更新挂售记录的出口 IP 和国家；
  - 在租约事件中记录 `egress_changed`；
  - 通知当前租户（新通知类型 `client_egress_changed`，国家变化时文案更醒目）。通知只发给租户，带完整的新旧 IP，同一网段内的变化也能看出（2026-11-27 调整，租户本来就能看到完整 IP）。
- 不因此自动退租，也不改变计费。国家变化时，租户可以按「供应商问题」退租，由现有争议流程处理。
- 公开卡片不展示变更次数（C2）。

### 7.7 验收

- 匿名用户只看到网段和国家；当前租户看到完整 IP，退租后不再可见。
- 排序：可租的在前，然后按接入时长；搜索和国家筛选的结果正确；URL 可以还原筛选状态。
- 30 天在线率与在线日历的数据一致。
- 模拟出口变化：一小时内租户收到通知，租约事件中有记录。

## 8. P6 买家体验

### 8.1 「我租的 Client」

登录且有进行中租约的买家，在 Client 市场页顶部看到一排租用卡片（数据来自 `leases?current=true` 和个人租用摘要）。每张卡片包含：
- 标题、国家与完整出口 IP；
- 状态：开通中 / 使用中 / 仅试用剩余 X 天 / 将被收回 / 退租中；
- 到期时间或下次扣费时间；
- 大号的「打开控制台」按钮；
- 次要操作：导出备份、续费设置、试用后选择、退租。

点击「详情」进入账户页的租用记录（保留现有页面）。下单成功后不再跳到账户页，而是留在市场页，并高亮这张新卡片。

### 8.2 开通引导

租约变为使用中以后，卡片上显示三步引导：
1. 打开控制台；
2. 添加你的上游账号；
3. 创建 Share 并复制 CLI 配置。

第 3 步由 Router 自动判断：租约下已有绑定的 Share（`online_client_market/shares.rs`）即视为完成，此时显示 Share 地址和复制按钮。第 1、2 步 Router 无法可靠得知，由租户手动勾选，勾选状态存在浏览器本地。三步都完成后，引导收起。

### 8.3 租用详情精简

`rental-records.tsx` 的详情改为两层：
- **常显**：状态、出口 IP、本期起止、价格、本期计费小时、操作。
- **折叠**到「账目与记录」：记录 ID、各时间点、清理状态、金额明细、账期列表、事件列表。

### 8.4 计费小时展示

- 个人租用详情和卖家库存新增本期的 `availableSeconds`、`unavailableSeconds` 和 `settledThrough`。来源是结算本身逐段写下的可用性记录（`market_platform_service_evidence`，`client_host_health`）：月租按 10/12 规则逐小时写入，试用期间按可用时间写入。直接读结算记录，双方看到的数字与实际扣费完全一致，包括首尾不满一小时的部分。
- 显示为「本期可用 X 小时，不可用 Y 小时（不收费）· 已结算至 {时间}」。免费租用没有结算记录，不显示。卖家的「已租出」卡片显示同样的数字。

### 8.5 风险说明

市场页顶部（只显示一次，可以关闭）和结账确认中显示：「Client 运行在卖家的机器上，卖家有能力读取机器上的数据。建议只放入可随时撤销、设有额度上限的账号或 API Key。」开通引导第 2 步也放这句提示。

### 8.6 验收

- 下单后留在市场页，新卡片高亮。「打开控制台」一步进入工作区。
- 创建 Share 后，引导第 3 步自动完成并显示 Share 地址。
- 租用详情默认只显示常用信息；折叠区里的数据与原来一致。
- 计费小时与结算结果一致。

## 9. P7 收尾

### 9.1 用词统一

- 清理 `frontend/lib/client-market-shop-messages.ts` 及相关词库：
  - 被租的东西统一称「Client」，例如「选购客户端」改为「选购 Client」，「管理主机」改为「管理 Client」；
  - 人统一称「卖家」「租户」，「主机所有者」改为「卖家」；
  - 只有指物理硬件时才用「机器」。
- 中英文同步修改，通知文案一并处理。

### 9.2 组件统一

- 原生的 `<select>`、`<textarea>`、复选框、`<details>`、`<ul>`（主要在 `lease-actions.tsx` 和 `online-market.tsx`）改为 Share 已经在用的组件（`CompactSelect`、HeroUI 的 Checkbox/TextArea、`ExpandableRecordTable`）。
- 焦点框统一为 `ring-accent`。现在混用了 `outline-primary` 和 `ring-accent` 两种写法。
- 状态文字改为状态标签，配色与 Share 卡片一致。

### 9.3 旧兼容代码清理（有前提）

可以删除的内容：
- 版本 1 租约的兼容逻辑（`management_version=1`，以及 Clients 卡片和租用详情里的「待迁移」提示）；
- 列表接口的 `includeAvailableHours24` / `availableHours24` 兼容查询；
- 原规划 §11 提到的旧 SSH 遗留。

前提：生产库中没有未释放的版本 1 租约和 SSH 租约，并且已经没有旧版前端在请求 `availableHours24`。需要先在生产环境核实，核实后再单独提交清理。

## 10. 兼容与上线

- 所有迁移只新增列、重建触发器或更新视图，不改写历史数据。上线前按惯例备份数据库。
- 新字段都有默认值：`afterTrial` 默认 `continue`；`title` 和 `paused_at` 可以为空；`tenantEmail` 可以为空。
- 前端是嵌入式静态资源，随 Router 二进制一起发布。
- 按本仓库约定，本机不运行测试。各阶段的测试用例随代码一起提交，由用户按发布流程执行。

## 11. 风险

| 项目 | 说明 |
|---|---|
| 收回与退租的竞态 | 依赖 `accept_tx` 以先写入者为准，以及同一事务内的检查，需要专门的测试。 |
| 立即收回被滥用 | 只靠「必须填写说明 + 记入审计」约束。如果出现滥用，再考虑限制次数或改为管理员审批。 |
| `client_market_terminations.installation_id` 实际存的是租约 ID | 写新查询时容易出错。 |
| 暂停状态进入供给视图 | 视图被商店、报价、地区统计、排名空闲数、准入多处共用，需要逐一确认 `paused` 的处理。 |
| 每小时出口检查的开销 | 每台 Client 每小时一次回调。规模变大后可以按租约状态调整频率。 |
| 标题内容 | 只做格式拦截，不做内容审核。出现问题时由管理员处理，可后续补「管理员清空标题」的操作。 |
| 列表接口返回卖家邮箱 | 与 C3 一致，但要确认与 Provider 身份的公开规则不冲突（Share 目录已有先例）。 |

## 附：IP 质量调研摘要（本轮不实施）

- **社区通行做法**：开源脚本 IPQuality 把 MaxMind、IP2Location、ipinfo、ipregistry、ipapi.is、AbuseIPDB、IPQS、DB-IP、Scamalytics 的结论并排列出，不合成总分。「原生 IP」按「注册国家 = 实际国家」判断。
- **免费额度**：ipapi.is 每天 1000 次，允许商用；Scamalytics 每月 5000 次；ipinfo 免费版没有类型字段；IP2Location 的类型字段需要付费，免费版要求署名。
- **AI 服务可用性的判断方法**：
  - Claude：访问 claude.ai，被重定向到 `anthropic.com/app-unavailable-in-region` 即不可用。
  - ChatGPT：`api.openai.com/compliance/cookie_requirements` 返回 `unsupported_country` 表示地区不可用；`ios.chat.openai.com` 页面出现 VPN 字样表示被识别为 VPN。
- **Anthropic 支持地区名单**（2026-11-12 生效）：不含中国大陆、香港、俄罗斯、伊朗。

## 实施状态

已实施 P0–P6 和 P7 的 9.1、9.2；9.3（旧兼容代码清理）按前提条件留待生产核实后单独处理。未运行测试或部署，只做了 `cargo check --tests` 和 `tsc --noEmit`。

| 阶段 | 落点 |
|---|---|
| P0 拆分 | `online-market.tsx` 只负责读取与组合；`shop-catalog.tsx`、`client-checkout.tsx`、`seller-workspace.tsx`、`my-rentals.tsx`、`client-listing-terms.tsx`、`client-reclaim-dialog.tsx`、`client-after-trial-dialog.tsx` |
| P1 | `store.rs` 快照按查看者过滤租户邮箱和租户 Share 的 owner 邮箱 |
| P2 | `schema/0131_client_market_seller_controls.sql`、`src/online_client_market/seller.rs`、`reconcile.rs` 的到期执行与收回后下架；收回说明写入退租记录（`client_market_termination.rs`） |
| P3 | 标题、`PATCH .../listing`、暂停与恢复、卖家历史表（含收入） |
| P4 | `market_platform_contracts.rs` 的 Client 仅试用上限与切换、`commit_quote` 的 `afterTrial` |
| P5 | 商店卡片、30 天在线率、排序、搜索、分页、详情抽屉；每小时出口复核与变化通知；完整 IP 只在租约使用中返回 |
| P6 | 「我租的 Client」、开通引导、租用详情两层结构、按结算记录展示的计费时间 |

2026-11-27 第二轮：按复核结论补齐了收入列、按结算记录计算的计费时间、收回说明对租户可见、收回时间不晚于已付租期、未开通订单的取消文案、完整 IP 只在使用中返回、上架确认中的 IP 可见说明、租用卡片的「以出口 IP 为准」提示和焦点框统一；并把以下调整写回了正文（§4.1、§4.5、§6.3、§7.6、§8.4）：

- 「试用后」弹窗不与 Share 共用组件。
- 未开通订单的收回立即生效。
- 「以后拒绝这个租户」在收回生效时写入。
- 取消收回后恢复租户原来的自动续费（原计划为保持关闭）。
- 出口变化通知带完整 IP。
- 收回通知期内，「试用后继续」临时改为仅试用，取消收回后恢复（复核发现：否则通知期内可能先扣首月）。
- 免费挂售改为收费时，与上架一样自动认领供应商身份；「以后拒绝」写入失败不会阻塞收回或租户退租。

仍保留的说明：「打开控制台」第 1 步在点击按钮时自动勾选，第 2 步由租户手动勾选。

## 2026-11-27 离线生命周期补充

身份恢复和 Owner 离线下架已由 [专门方案](client-identity-recovery-and-offline-unlisting.md) 补齐。市场下架立即停止新订单，不等待本机解除锁定，也不终止已有租约；恢复出租仍要求租约和清理安全条件满足。
