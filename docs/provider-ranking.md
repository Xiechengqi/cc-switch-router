# Provider Rank v1

Provider Rank 是 Router 内建 Share Market 与 Client Market 的供应商发现层。它把同一供应商跨两个市场的供给、履约和资金关系归到一个稳定身份，并用可审计的观测信号生成排名。它不改变商品准入、授权、计费或结算规则。

## 术语与边界

- **Market Provider** 是市场供应商主体，公开 ID 为随机生成且稳定的 `mp_<opaque>`。它可以同时拥有 Share listing、Client Host、预付/信用关系和服务合同。
- Server 中的 Claude、OpenAI、Gemini 等 **upstream Provider type/profile** 是模型运行配置，不是 Market Provider。两者不得通过名称、邮箱或 Provider type 相互推断。
- `market_provider_profiles.canonical_email`、`user_id` 和 aliases 只用于 Router 内部身份归并；公开排名 API 不返回这些字段。
- 官方身份只说明 Router 运营方是谁，不增加分数、不改变正式名次。探索展示也不改变 `rankPosition`。

## 稳定身份

Migration 52 建立 `market_provider_profiles` 和 `market_provider_aliases`，并为 Host Provider 与 Share listing 写入同一个 `market_provider_id`。`market_provider_share_windows` 另外保存 listing 每次开放期间的 Provider 归属；关闭、重开或转移所有者都会在同一事务内结束/开启半开时间窗，因此当前归属变化不会重写历史证据。身份发现覆盖：

- Host Provider 与 Share listing；
- Market counterparty、信用账户和预付账户；
- legacy service contract 与 calendar-month recurring contract。

已验证用户通过 `user_id` alias 认领同邮箱的未认领身份时，原公开 ID 保持不变。邮箱、用户或 Host alias 指向多个身份时，Router 将相关身份标记为 `identity_conflict` 并停止猜测；私有资金解析也会 fail closed。供应商写路径若因冲突回滚业务事务，会在回滚后用独立事务持久化身份 fence 和审计事件，既不误提交付款资料、Host 或 listing 变更，也不会让错误响应抹掉冲突证据。GET 接口不做写时修复，身份归并只发生在迁移、供应商写路径和后台 reconciliation 中。

## 排名信号

正式分数范围是 0–10000 basis points：

```text
score = 45% service quality
      + 30% effective choice
      + 15% fulfillment
      + 10% supply breadth
```

只有同时达到 **3 个独立买家**和 **7 个观测日**的 Provider 才进入 `ranked`；其余 Provider 保持 `collecting`，仍在列表中展示。

### Service quality（45%）

- Share 使用最近最多 400 天的 canonical model-probe observation；legacy/unobserved 证据不进入成功样本。探测、请求性能、有效选择和履约都按事件发生时所在的显式 listing Provider 时间窗归属；关闭期间不计，重开或换主也不会把旧证据移动给当前 Provider。
- observation 必须通过 `share_model_health_slots.observation_id` 实际投影到对应 Share，才可沿该 Share 的 listing 时间窗归属；同一 observation 只计一次，不因同一 Share 的历史 listing 或同 installation 的其他 Share/listing 重复、串号或放大样本。
- Client 使用最近 30 天的 Host online/observed samples。
- 两类信号同时存在时按 70% Share、30% Client 混合；单边存在时使用该单边。
- 比率使用 95% Wilson lower bound，避免一个稀疏的 100% 样本压过长期稳定供给。

### Effective choice（30%）

- 使用最近 90 天已实际激活且不属于自租的 Share/Client 选择。
- 付费选择权重为 1.00；免费选择权重为 0.25。
- 同一买家跨 Share 与 Client 选择同一 Provider 只保留一个买家，并取较高权重。
- 未激活、一天内释放和 Provider 自租不计入选择；自租按事件所属时间窗的 Provider user ID 与 canonical email fail closed。交易行上的 owner 字段会在合法换主流程中更新，因此不能作为历史归属真相。
- “独立买家”在 v1 中表示不同的 Router `user_id`，不是 KYC 身份或完整的反女巫证明。

### Fulfillment（15%）

- Share 的已激活 subscription 算成功，`grant_failed` 算失败；仍在 pending 的授权不进入分母。
- Client 的 create provisioning job 仅以 `succeeded` / `failed` 进入分母。
- 两个市场的 Provider 自租/自建都不进入履约分子或分母，避免供应商用自测请求堆高样本。
- v1 使用全部已保存的履约历史；quality 和 choice 使用上述时间窗口。这是有意的保守口径，后续如改为滚动窗口必须提升 algorithm version。

### Supply breadth（10%）

已启用 App 数、国家数以及可用 Share seat/空闲 Host 数形成饱和分数。Share 供给必须同时满足 listing 有效、底层 Share active 且 owner 一致、至少存在一个未退休可见 seat；只有实际 enabled 的 App binding 计数。Client 已 disabled 的 Host 不再属于公开供给。以上边界与两个 Market 的可选库存一致。供给广度不能代替质量和履约。

TTFT 与 TPS 来自最近 30 天成功、非健康检查且完整结束的流式 Share 请求；TPS 还要求 observed usage 和可信的生成时长，terminal flush 不进入吞吐样本。同一请求只计一次，不因 listing 历史重复加权。两项指标仅作展示，`displayOnly=true`，不参与分数、资格或排序。

## 代次、发布与陈旧语义

`market_provider_rank_generations` 和 `market_provider_rank_entries` 构成不可变代次：

1. 后台任务启动后立即 reconciliation 并生成首个代次，之后每 15 分钟刷新。
2. 一个 `IMMEDIATE` 事务内先按真实时间值校验 Share 时间窗可解析、结束不早于开始、窗口无重叠且当前生命周期一致，再写完 `building` generation、所有 entries，最后原子切换为 `published`；开窗和关窗写路径也拒绝时钟回拨产生的反向边界，账本漂移会 fail closed。
3. 生成失败只写脱敏的 `failed` 诊断；`building`、`failed` 和部分数据永远不会替换最新 `published` 代次。
4. 没有已发布代次时公开 API 返回 503；已发布代次超过 30 分钟时继续返回 last-good，并标记 `generation.stale=true`。
5. 只为当前仍有活跃 Share listing 或 Client Host 的身份生成公开排名；纯资金/历史合同身份只出现在相应买家的私有资金区。成功发布时清理 30 天前的旧代次，entries 由外键级联删除，当前代次始终保留。

排名生成不再阻塞 Router 监听器启动。大库启动时，Provider 页面可能短暂处于“排名尚未准备好”，但其他 Router 能力保持可用。

## 推荐发布开关

`CC_SWITCH_ROUTER_MARKET_PROVIDER_RECOMMENDATION_MODE` 支持：

- `off`：保留旧顺序；
- `shadow`（默认）：计算并记录将发生的位移，但响应保持旧顺序；
- `on`：Share 推荐顺序和 Client Provider 选择顺序使用正式名次。

Share 页的价格、空闲量和在线率等显式排序始终使用原语义；Provider Rank 只影响“推荐”。Client 的 Host 明细仍按 Host 自身条件展示，Provider Rank 主要影响创建 Client 时的 Provider 选择。官方身份在 `off/shadow` 中保留旧版兼容顺序，在 `on` 中不作为 tie-breaker。

## API 与缓存

| 方法 | 路径 | 可见性 | 缓存 |
|---|---|---|---|
| `GET` | `/v1/market-providers` | 公开排名、组件分、供给摘要、支付方式种类 | `public, max-age=60, stale-while-revalidate=300` + strong ETag |
| `GET` | `/v1/market-providers/:id` | 公开 Provider 详情及 Share/Client 供给 | 同上 |
| `GET` | `/v1/market-providers/me/funding` | 当前登录买家的供应商资金关系 | `private, no-store`，无 ETag |

公开响应不得包含 canonical email、Router user ID、supplier user ID、预付余额、信用或合同金额。公开 display name 若为空或含邮箱会退回稳定匿名名；Provider 详情中的 Share 名称也由 listing ID 派生为匿名标签，避免历史 `share_name` 实际保存 owner email 时旁路泄漏。私有 funding 响应可以包含当前买家关系所需的 supplier identity，并严格按 Session buyer 过滤。

## 资金汇总

预付与信用账户按「买家 + 供应商」共享，Share/Client 两个 funding projection 中的余额和信用只汇总一次。funding projection 的 active rate 本身是跨产品共享口径，因此 Provider 页面另按合同 `product_kind` 计算 Share/Client legacy daily run rate，避免把同一总额重复相加；calendar-month commitment 按合同相加。最近续费金额只合计最早同一续费时间的合同。无限信用显示为 Provider 数量，不伪装成金额。

Provider 页面仅在用户已登录后请求私有资金端点。即使 Provider 当前没有公开供给，只要与当前买家存在预付、信用或合同关系，仍会出现在资金区。Provider 级充值入口复用既有 Binance funding 流程，并在 Share/Client 即时覆盖需求与最早一批尚未备资的 calendar-month 续费之间选择更高的资金需求；已被 hold 覆盖的自动续费不会再次建议充值。

## 前端导航

`/providers/` 与 Client、Share Market、Client Market、账户同级。公开详情 Drawer 使用 `/providers/?provider=<opaque-id>`；跳转市场时分别使用 `/share-market/?provider=<opaque-id>` 和 `/client-market/?provider=<opaque-id>`。目标市场只匹配服务端返回的 `marketProviderId`，未绑定旧记录不会被猜测归入筛选结果。

## 当前扩展限制

- 业务库仍由单个 `Arc<Mutex<Connection>>` 串行访问；排名刷新会在事务期间占用连接。首轮已移出同步启动路径，但大规模市场仍应监控刷新耗时。
- v1 candidate 计算按 Provider 聚合多个历史表。若 Provider 数和观测历史继续增长，应先引入物化日汇总，再提高刷新频率；不能通过减少证据校验换取速度。
- 推荐开关是 Router 级动态设置，不是每用户实验。修改为用户级实验前必须定义稳定分桶与可审计 exposure。
- v1 只能排除直接自租，不能证明多个 Router 账户背后一定是不同自然人。若排名开始影响更高价值的流量或结算，应在提升算法版本前引入付款、设备或风控侧的抗女巫证据，不能把 `user_id` 数量当作已验证身份数。
