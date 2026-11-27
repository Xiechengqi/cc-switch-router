# Share Market 全站租用审计

## 范围

仅 Router owner 可用的 Share 租用审计，与 [Client Market 全站租用审计](client-market-rental-audit.md) 同构。一条记录对应一次已提交的 Share 租用（`share_market_subscriptions` 的一行），包括未成功开通、试用、免费、已结束的租用。续费不增加主记录，各付费周期在详情中独立分页。审计只读，没有资金操作、删除或审批入口。

## 使用

「租用审计」是 Share Market 顶部的一个标签，位于「我的挂售」右侧，仅 Router owner 可见（`session.canAuditClientMarket`，依据 `router_owner_email`，与 Client 审计同一能力）；链接 `?view=audit`，无权限时回到「选购拼车位」。切到该标签才读取数据，离开即卸载、取消读取并清除内存中的数据；审计标签期间暂停市场轮询。表格、筛选、页码跳转和详情弹窗与 Client 审计共用 `frontend/components/market/rental-audit-workspace.tsx`；主行显示 Share 与车位、租用者、状态／后续事项、实扣租金和下单时间。详情可打开关联的退租处理弹窗（Owner 可读）。

## 身份快照

迁移 133 新增 `share_market_rental_identities`，每个 subscription 一行：Share ID、名称、子域名、应用类型、listing／seat ID、seat 序号、installation ID、买卖双方用户 ID 与邮箱、下单时间（统一为 UTC 毫秒 RFC3339）和来源 `provenance`。

- `order_snapshot`：`AFTER INSERT ON share_market_subscriptions` 触发器在同一语句内写入，覆盖所有下单路径，不依赖 Rust 代码。
- `historical_record`：迁移时回填。退租载荷中冻结的 Share 名称、子域名和双方邮箱优先于今天的 Share 行；应用类型取租用冻结的 `required_app`。Share 所有权转移会改写在租 subscription 的 Provider 字段，因此 Provider ID 以平台合同为准；无法证明的 Provider 邮箱留空。已被删除的 Share 的名称、子域名、应用为 NULL，不推测。

`shares` 可以被硬删除；快照表不建外键，禁止更新和删除，审计不依赖 Share 当前是否存在（`sharePresent` 只是展示）。

## 状态

`status`（优先级从上到下）：

- `grant_failed`：subscription 为 `grant_failed`，或合同 `activation_failed`，或未激活即 `released`。
- `ended`：subscription 已 `released`（subscription 缺失时以合同 `ended` 为准）。
- `ending`：`revoke_pending`／`revoke_failed`，或合同 `ending`／`ended` 但访问尚未释放，或已有退租记录。
- `paused`：`billing_*` 计费暂停相关状态，或合同 `suspended`／`review_required`。
- `opening`：`grant_pending` 或合同 `pending_activation`。
- `trialing`／`paid`：由平台合同 `trialing`／`active` 判定，subscription 的 active 状态本身不代表付费。
- `free`：无合同、冻结定价为 free 且处于 active。
- `unknown`：其他情况，例如没有平台合同的旧付费租用。

`attention`（优先级从上到下）：`finance_review`（退租 finance_pending 或合同 review_required）、`disputed`（退租 disputed 或责任未定）、`revoke_failed`（subscription revoke_failed，或最新撤销操作被拒绝且未释放）、`revoke_pending`（撤销进行中）、`billing_control_failed`、`settlement_pending`（退租未完成，或租用已结束但合同未结束）、`none`。查询参数 `attention=any` 表示非 none。

## 金额

先分页，再对当前页合同批量聚合，各来源独立分组，避免一对多连接造成倍增。金额为整数账本单位字符串。

- 实扣：已扣款周期 `market_platform_periods.amount_units` 加按量结算批次。
- 返还：周期 `refunded_units`。
- 剩余预付：周期金额减时间交付、返还、退租补偿和配额交付（`share_quota_units`）。争议暂留属于剩余预付，不是额外扣款。
- 服务交付、Provider 净收益、平台抽成、已转入余额：`contract_kind='share'` 的交付切片与收益批次（含立即退租时的配额交付）。
- 退租补偿：`share_termination_income`（无冲正列，毛额即补偿）；`credited_units` 为已成熟入账部分。
- 没有平台合同且非免费的租用 `money` 为 null（历史财务证据不足），不伪装为零。

## 接口

两个接口每次调用 `require_owner_session`（依据 `router_owner_email`，不是 `isAdmin`），响应为 `private, no-store`。

- `GET /v1/admin/share-market/rental-audit?search&buyerEmail&providerEmail&status&attention&from&until&page`：每页固定 5 条，按下单时间、ID 降序；超出范围的页码校正为最后有效页，未知参数拒绝。`search` 匹配 subscription／Share／seat／installation ID 前缀、Share 名称或子域名前缀（转义 LIKE），以及完整买卖方邮箱。
- `GET /v1/admin/share-market/rental-audit/:id?periodCursor&eventCursor`：详情、付费周期（每批 50 条，游标为最后序号）、事件（每批 50 条，游标为最后事件 ID）、试用条款 `trial` 和退租结算 `settlement`。

事件来自 `share_market_events`、`share_termination_events` 和合成的 `order_committed`。动作名按白名单输出（退租动作与 Client 审计同名），其他为 `updated`。不返回原始 `detail_json`；只有人工补偿决定的备注可见。退租事件的操作者只在等于冻结的买卖方时显示邮箱。

## 验证

Rust 测试覆盖分页与末页校正、筛选校验、删除和改名 Share 后快照保留、合同判定试用／付费、退租与撤销的后续事项、多切片与补偿不倍增、事件分页不泄露原始数据以及分页前不读金额表。测试只编译（`cargo check --tests`），未运行。
