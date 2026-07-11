# Router UI/UX 默会知识导向落地实施计划

状态：工程实施完成，进入持续观测

制定日期：2026-07-11

理论框架：Michael Polanyi 的默会知识、`from-to` 结构、辅助意识/焦点意识与寄居（indwelling）
适用范围：`cc-switch-router` Web 前端及为状态解释、交互联动所需的 Router Dashboard 数据

## 1. 目标

本计划的目标不是继续增加 Dashboard 字段，而是让用户形成稳定、可复用的操作直觉：

1. 不逐项阅读数字，也能快速判断当前请求是否正常。
2. 从地图或列表感知异常后，最多两次交互定位到 Client、Share 或 Market。
3. 将颜色、位置、运动、密度等辅助线索稳定地指向同一个系统结论。
4. 用户聚焦某个实体时，地图、Client/Share 和 Market 提供一致证据。
5. 详情视图先解释“为什么”，再展示完整技术字段。
6. 轮询刷新不破坏用户已形成的空间记忆和当前操作上下文。

目标操作闭环：

```text
感知系统态势 → 聚焦实体 → 验证原因 → 执行操作 → 观察恢复
```

## 2. 设计原则

### 2.1 辅助意识保持安静

- 健康连接、健康实体和正常数值保持低视觉权重。
- 颜色、运动、位置和密度共同表达状态，不依赖单个 Badge。
- 默认界面只突出当前最重要的异常原因。
- 技术细节继续存在，但不与主要判断争夺注意力。

### 2.2 焦点意识必须一致

- 任一时刻最多存在一个主焦点：Request、Client、Share 或 Market。
- 所有 Dashboard 区域读取同一个焦点状态。
- 聚焦不等于隐藏：无关对象降低权重，但仍保留系统整体上下文。
- 用户可以通过焦点条、`Esc` 或点击空白明确退出焦点。

### 2.3 稳定性优先于瞬时排序

- 状态、颜色、字段顺序、节点位置和交互含义保持稳定。
- 轮询更新不得重置折叠、筛选、横向滚动、抽屉或焦点。
- 状态变化可以立即标记，但不应导致实体每 5 秒频繁换位。
- 用户正在操作的实体固定位置，直到操作结束或主动退出焦点。

### 2.4 明确知识用于验证，而不是替代感知

- 地图负责请求流和整体态势。
- Client/Share、Market 负责实体定位和横向比较。
- 抽屉负责状态原因、影响范围、时间和支撑证据。
- 日志和技术字段负责最终诊断。

## 3. 当前基线

当前 Dashboard 已具备以下基础：

- 地图默认展开，支持 Router→Client 连接、实时请求流、节点状态和请求 ticker。
- Client→多个 Share 的层级正确，Share 使用横向浏览。
- Client、Share 和 Market 均有详情抽屉。
- Client/Market 已具备搜索、状态筛选、异常筛选和排序。
- Client 支持收款摘要、折叠状态保存和聚合状态。
- Share 卡片已压缩为核心指标加一个主要异常。
- Market 已使用桌面紧凑表格进行横向比较。
- Dashboard 每 5 秒刷新数据。

当前主要缺口：

- 地图、Client/Share、Market 之间尚无统一焦点模型。
- 状态原因主要由前端分散推断，缺少统一、可验证的原因编码。
- 状态变化可能立即改变异常优先排序，影响空间记忆。
- 抽屉仍偏向字段集合，没有统一的“结论→原因→影响→证据”结构。
- 缺少用于验证真实操作路径的匿名本地交互指标。
- 高流量时地图逐请求动画可能从有效线索变成视觉噪声。

## 4. 范围边界

### 4.1 本计划包含

- 统一 Client、Share、Market 的运行状态与原因模型。
- Dashboard 共享焦点状态和跨区域联动。
- 地图内部请求流、聚焦、残留和高流量聚合优化。
- Client/Share、Market 的焦点反馈与稳定排序。
- 详情抽屉的信息解释顺序。
- 操作结果与请求恢复反馈。
- 用户偏好和操作上下文保存。
- 本地、最小化、无敏感信息的交互观测。
- 桌面端键盘操作和可访问性。

### 4.2 本计划不包含

- 导航栏重构。
- 地图移动、默认折叠或降低地图优先级。
- 移动端和平板端适配。
- Client、Router 或 Market 历史数据迁移。
- 修改实际请求调度算法。
- Market 充提和链上钱包重构。
- 新的支付或结算业务逻辑。
- 将行为数据发送到第三方分析平台。

## 5. 统一状态与原因数据契约

### 5.1 原则

状态结论必须由 Router 提供规范化结果，前端负责展示，不应由多个组件分别推断。已有原始字段继续保留，新增字段采用向后兼容方式加入 `/v1/dashboard`。

建议在 `DashboardClient`、`ShareView`、`DashboardMarket` 中增加：

```ts
type OperationalState =
  | "available"
  | "online"
  | "degraded"
  | "offline"
  | "maintenance"
  | "disabled";

type OperationalReasonCode =
  | "route_offline"
  | "health_check_failed"
  | "no_online_shares"
  | "partial_share_outage"
  | "parallel_capacity_full"
  | "parallel_capacity_warning"
  | "usage_limit_warning"
  | "expired"
  | "expires_soon"
  | "provider_unavailable"
  | "high_latency"
  | "edit_pending"
  | "edit_failed"
  | "maintenance_enabled"
  | "manually_disabled";

type OperationalReason = {
  code: OperationalReasonCode;
  severity: "info" | "warning" | "critical";
  startedAt?: string;
  entityType?: "client" | "share" | "market" | "provider";
  entityId?: string;
  currentValue?: number | string;
  threshold?: number | string;
};

type OperationalSummary = {
  state: OperationalState;
  primaryReason?: OperationalReason;
  additionalReasonCount: number;
  changedAt?: string;
};
```

### 5.2 状态聚合规则

#### Client

1. Client tunnel 和所有启用 Share 均不可用：`offline`。
2. 至少一个启用 Share 在线，但存在离线或健康检查失败的启用 Share：`degraded`。
3. 所有启用 Share 在线且健康：`online`。
4. 停用 Share 不使 Client 降级。
5. 所有 Share 均主动停用时，使用 Client tunnel/heartbeat 判断，不将主动停用误报为故障。

#### Share

1. 主动暂停、过期和禁用保持独立原因。
2. 启用但路由不可用：`offline`。
3. 路由可用但健康检查、容量、用量、延迟或 Provider 异常：`degraded`。
4. 路由和 Provider 正常：`online`。

#### Market

1. 管理员禁用：`disabled`。
2. 主动维护：`maintenance`。
3. 心跳或路由不可用：`offline`。
4. 无在线 Share、容量达到告警阈值或健康检查失败：`degraded`。
5. 其余状态：`available`。

### 5.3 主要原因优先级

```text
路由不可用
→ 配置应用失败
→ 已过期/即将过期
→ Provider 不可用
→ 并发容量已满
→ 用量接近上限
→ 健康检查失败
→ 高延迟
→ 主动维护/禁用等信息状态
```

前端默认展示 `primaryReason`，并以 `+N other issues` 引导进入详情。

### 5.4 涉及文件

- `src/models.rs`：新增序列化结构。
- `src/store.rs`：在 Dashboard 聚合阶段生成状态和原因。
- `frontend/lib/types.ts`：增加前端类型。
- `frontend/lib/i18n.ts`：原因编码对应的中英文文案。
- `frontend/components/dashboard/share-dashboard-utils.ts`：只保留格式化和兼容回退逻辑。

## 6. Dashboard 焦点状态架构

### 6.1 共享状态

新增 `DashboardFocusProvider`，由 `DashboardPage` 统一提供：

```ts
type DashboardFocus = {
  target: null | {
    kind: "request" | "client" | "share" | "market";
    id: string;
    source: "map" | "client-board" | "market-table" | "drawer" | "activity";
  };
  relatedClientIds: string[];
  relatedShareIds: string[];
  relatedMarketIds: string[];
  setFocus(target: DashboardFocus["target"]): void;
  clearFocus(): void;
};
```

### 6.2 焦点行为

| 来源 | 主焦点 | 地图 | Client/Share | Market |
|---|---|---|---|---|
| 点击地图 Client | Client | 突出节点及相关流量 | 定位、展开并高亮 | 保持全局，仅关联 Market 弱提示 |
| 点击 Share | Share | 突出所属 Client 和当前请求 | 卡片高亮 | 突出关联 Market |
| 点击 Market | Market | 突出近期相关请求 | 标记关联 Share | 行高亮 |
| 点击实时请求 | Request | 固定该请求路径 | 突出命中 Share | 突出命中 Market |

### 6.3 焦点条

地图下方或 Dashboard 内容区顶部增加紧凑焦点条，不改导航栏：

```text
Viewing: Share “OpenAI Official” · Client sg-01 · 2 Markets     [Clear ×]
```

要求：

- 始终明确当前为什么被过滤或弱化。
- `Esc` 清除焦点。
- 打开/关闭抽屉不自动清除焦点。
- 数据刷新后实体仍存在则保留焦点；实体消失则清除并显示一次提示。
- 焦点状态写入 URL query，允许刷新和复制链接后恢复；短期 UI 状态仍保存在 session/local storage。

### 6.4 涉及文件

- 新增 `frontend/components/dashboard/dashboard-focus.tsx`。
- 新增 `frontend/components/dashboard/focus-bar.tsx`。
- 修改 `frontend/components/dashboard/dashboard-page.tsx`。
- 修改 `live-map.tsx`、`client-board.tsx`、`share-card.tsx`、`markets-table.tsx`。

## 7. 地图内部优化计划

地图保持当前位置、默认展开和现有高度。只优化地图内部信息语法与联动。

### 7.1 请求流视觉层级

- 正常非焦点路径：低对比度、短残留。
- 当前焦点路径：最高对比度，并显示方向。
- 失败请求：保留时间长于成功请求。
- 高延迟请求：使用独立的警告色，不与失败共用红色。
- 离线节点：停止活跃动画，保留明确文字状态。
- 无焦点时维持全局态势；有焦点时其他路径降低权重但不消失。

### 7.2 高流量聚合

按最近滑动时间窗口计算同路径请求密度：

- 低流量：逐请求动画。
- 中流量：合并同路径请求，缩短动画持续时间。
- 高流量：以路径宽度、亮度和计数表达，不逐条闪烁。

阈值根据浏览器渲染性能和实际请求量压测确定，不直接硬编码业务判断。

### 7.3 请求残留

- 成功请求淡出时间短。
- 失败、高延迟请求残留时间长。
- 只保留有限数量的历史轨迹，避免内存持续增长。
- 暂停浏览器标签页后不补播全部历史动画，恢复时直接使用最新状态。

### 7.4 空间稳定性

- Client 节点坐标只由地理信息或稳定回退规则决定。
- 同一数据刷新周期内不重新随机定位。
- 无地理坐标的节点使用稳定散列位置。
- 聚焦和退出焦点不得改变节点位置。

## 8. Client/Share 计划

### 8.1 稳定异常排序

当前异常优先排序增加迟滞机制：

- 新异常立即改变状态视觉，但经过稳定窗口后才移动到异常分组。
- 恢复后同样经过稳定窗口再回到正常分组。
- 当前聚焦、抽屉打开或正在操作的 Client 不移动。
- 同一状态分组内使用稳定注册顺序或用户选择的排序。

### 8.2 Client 状态解释

Client 头部状态支持快速原因说明：

```text
Degraded
1 of 6 enabled Shares is offline · since 2m ago
```

- 默认只显示聚合状态。
- 悬停或聚焦状态显示主原因和持续时间。
- 点击状态进入 Client 抽屉的诊断区域。
- 收款摘要继续保持中性，不参与健康状态颜色。

### 8.3 Share 卡片

- 保持当前固定字段顺序和横向滚动结构。
- 只突出一个主要原因。
- 其他原因显示数量，不增加多个并列 Badge。
- 聚焦 Share 时自动滚动到可见区域。
- 不自动改变用户手动选择的横向滚动位置，除非焦点来自外部区域。
- Share 与 Market 联动时，卡片显示轻量关联标识，不展开完整 Market 列表。

## 9. Market 计划

### 9.1 固定比较结构

保持以下列及位置：

```text
Market | Status | Capacity | Activity | Shares | Health | Updated | Actions
```

- 不因 Market 类型动态交换列。
- 未知容量显示 `Unknown`，不显示 `0/0`。
- 主动维护和禁用不计入故障告警，但在状态筛选中可见。
- 容量、活动和健康原因使用统一状态数据契约。

### 9.2 轻量趋势

在不增加新列的前提下，为容量或活动加入小型趋势线：

- 默认展示短时间窗口的相对变化。
- 趋势线没有坐标轴，不替代 Metrics 页面。
- 只用于判断“和平时是否不同”。
- 无足够样本时不渲染伪趋势。

### 9.3 与 Share 联动

- 聚焦 Market 时，关联 Share 使用统一高亮样式。
- 聚焦 Share 时，Market 表突出服务该 Share 的行。
- 联动只改变视觉权重，不暗中改变用户的筛选条件。
- 如需要真正过滤，必须显示明确过滤提示并支持一键清除。

## 10. 详情抽屉重构

Client、Share、Market 抽屉统一使用以下信息顺序：

```text
状态结论
主要原因与持续时间
影响范围
最近变化
支撑证据
完整配置与操作
```

### 10.1 公共诊断摘要组件

新增 `OperationalDiagnosis`：

```text
Degraded
Parallel capacity reached 10/10
Since 2 minutes ago
Impact: new requests may route to another Share
+2 other issues
```

组件要求：

- 使用原因编码生成稳定文案。
- 显示原因首次出现时间，而不是仅显示最后刷新时间。
- 区分系统推断、用户主动操作和自声明数据。
- 支持跳转到对应证据区块。

### 10.2 抽屉内分区

- Client：Diagnosis / Shares / Providers / Payout / Technical details。
- Share：Diagnosis / Provider / Markets / Requests / Configuration。
- Market：Diagnosis / Shares / Capacity & priority / Requests / Maintenance。

不要求一次性引入复杂 Tabs；优先使用锚点式分区和固定摘要头，验证内容规模后再决定是否使用 Tabs。

## 11. 操作反馈与恢复验证

所有改变路由状态的操作必须同时反馈：

1. 操作是否成功。
2. 对新请求的预期影响。
3. 系统是否观察到恢复或生效。

示例：

```text
Share disabled
New requests will no longer route to this Share.
```

```text
Routing recovered
First successful request completed 3 seconds ago.
```

实现要求：

- 提交成功不等同于运行恢复。
- 操作 Toast 只说明提交结果。
- 实际恢复由后续 Dashboard 状态或成功请求验证。
- 超时未验证时显示“配置已提交，尚未观察到恢复”，不得误报成功。

## 12. 用户偏好与上下文保存

### 12.1 Local storage

保存：

- Client 折叠状态。
- 用户选择的排序方式。
- 地图图层偏好。
- Market/Client 筛选器的可选长期偏好。

### 12.2 URL query

保存适合分享和刷新恢复的状态：

- 当前主焦点。
- 当前打开的实体详情。
- 明确的实体过滤条件。

### 12.3 不保存

- API Key、完整 Token、私密请求内容。
- 临时复制结果和 Toast。
- 已消失实体的焦点状态。
- 可能泄漏 Owner 收款地址的外部分析参数。

## 13. 观测与用户研究

### 13.1 本地交互事件

只记录完成 UX 验证所需的事件类型和匿名实体类型，不记录邮箱、地址、Token、请求正文或完整 URL：

```text
dashboard_focus_set
dashboard_focus_clear
map_request_selected
client_located_from_map
share_located_from_request
market_located_from_share
drawer_opened
diagnosis_evidence_opened
filter_applied
operation_submitted
operation_verified
```

必要字段：

- 事件时间。
- 来源区域。
- 目标实体类型。
- 交互步数。
- 距离前一关键事件的耗时。
- 是否通过键盘完成。

数据只进入 Router 本地存储，提供关闭开关和保留期限。

### 13.2 研究方式

- 情境观察：记录用户真实排障的视觉和点击顺序。
- 关键事件访谈：选择真实故障，询问最早察觉异常的线索。
- 刺激回忆：回放操作记录，让用户解释关键转折点。
- 专家/新手对照：比较路径、耗时和误判原因。
- 避免把持续 Think Aloud 作为唯一依据，以免改变原本默会的判断过程。

## 14. 分阶段实施

### 阶段 0：建立基线和冻结语义

任务：

- 列出当前 Client、Share、Market 全部状态组合。
- 为真实异常建立最小测试数据集。
- 记录现有定位路径、操作步数和刷新行为。
- 确认容量、用量、延迟和过期告警阈值来源。
- 确认地图在低、中、高流量下的性能基线。

交付物：

- 状态语义表。
- 原因编码表。
- UX 基线数据。
- 测试数据夹具。

退出条件：

- 每种状态都有唯一含义和明确负责的数据源。
- 主动状态与故障状态不混用。

### 阶段 1：统一状态契约

任务：

- 后端生成 `OperationalSummary`。
- 前端类型和中英文原因文案落地。
- Client、Share、Market 切换为统一状态数据。
- 保留前端回退计算，用于兼容开发过程中的旧响应。
- 增加聚合规则单元测试和序列化测试。

退出条件：

- 同一实体在卡片、表格、地图和抽屉中状态完全一致。
- 每个 Degraded/Offline 状态都能提供主要原因。

### 阶段 2：Dashboard 焦点模型

任务：

- 新增 `DashboardFocusProvider` 和焦点条。
- Client、Share、Market 支持统一选中和弱化样式。
- 实现 `Esc`、清除按钮和 URL 恢复。
- 保留轮询前后的焦点、筛选、折叠和滚动状态。
- 增加焦点状态单元测试与组件集成测试。

退出条件：

- 一个焦点在所有区域具有一致含义。
- 数据刷新不丢焦点。
- 实体消失时能够安全清理焦点。

### 阶段 3：地图联动与高流量表达

任务：

- 地图 Client、实时请求接入焦点模型。
- 实现请求路径与 Client/Share/Market 的双向定位。
- 增加成功、失败、高延迟的差异化残留。
- 实现同路径请求聚合和渲染上限。
- 保持地图位置、默认展开和空间稳定。
- 在真实请求回放数据上进行性能压测。

退出条件：

- 地图异常最多两次交互可定位到实体。
- 高流量场景无持续闪烁和明显掉帧。
- 聚焦前后地图节点位置不变化。

### 阶段 4：诊断抽屉与操作验证

任务：

- 新增公共诊断摘要。
- 重排 Client、Share、Market 抽屉内容。
- 将原因连接到健康检查、请求、Provider 或容量证据。
- 区分配置提交成功和运行恢复。
- 增加操作影响提示和恢复反馈。

退出条件：

- 用户不需要自行组合多个字段推导状态原因。
- 操作后可以明确知道是否已生效、是否已恢复。

### 阶段 5：熟练化与观测

任务：

- 增加安全的本地交互观测。
- 保存排序、筛选和地图图层偏好。
- 增加 Market 轻量趋势。
- 完成专家与新手任务测试。
- 根据真实路径删除无价值提示，而不是继续叠加帮助内容。

退出条件：

- 核心定位时间达到验收目标。
- 熟练用户不被教程和重复确认打断。
- 新用户能根据原因和反馈完成基本诊断。

## 15. 测试计划

### 15.1 单元测试

- Client/Share/Market 聚合状态矩阵。
- 主要原因优先级。
- 告警阈值边界。
- 主动禁用、维护不计为故障。
- 焦点关联实体计算。
- 稳定排序和迟滞时间窗口。

### 15.2 组件集成测试

- 地图选中 Client 后 ClientBoard 定位。
- Share 聚焦后 Market 行高亮。
- Market 聚焦后关联 Share 高亮。
- `Esc` 清除焦点。
- 轮询更新后焦点、筛选和抽屉保持。
- 状态原因与抽屉证据链接一致。
- 键盘可以完成聚焦、打开详情和退出。

### 15.3 端到端场景

1. 单个 Share 离线。
2. Client 下部分 Share 离线。
3. 全部 Share 主动停用。
4. Provider 不可用但 Share 路由仍在线。
5. Market 并发容量达到 70%、90%、100%。
6. Market 维护和管理员禁用。
7. Share 即将过期和已经过期。
8. 配置应用待处理、失败和成功恢复。
9. 高流量、突发失败和高延迟请求。
10. 聚焦实体在刷新过程中消失。

### 15.4 性能目标

- 5 秒轮询不造成整页明显重排。
- 地图动画不阻塞 Client/Market 交互。
- 大量 Share 横向浏览保持流畅。
- 聚焦状态更新只影响相关组件。
- 所有实时列表设置明确的数量和内存上限。

## 16. 验收指标

### 16.1 任务指标

- 熟练用户 3 秒内判断当前请求流是否正常。
- 从地图发现异常后，最多两次交互定位到具体实体。
- 从 Share 定位关联 Market 不需要手工复制 ID 或再次搜索。
- 状态原因在一次详情打开内可见。
- 轮询刷新不使用户丢失当前位置。

### 16.2 一致性指标

- Client、Share、Market 相同状态使用相同文字、图标和颜色语义。
- 状态 Badge、主要原因、详情证据无矛盾。
- Disabled 和 Maintenance 不被统计为故障。
- 颜色不是唯一状态载体。

### 16.3 熟练化指标

- 熟练用户完成任务的中位交互步数不增加。
- 新用户在查看状态原因后可以复述主要影响。
- 用户不需要记忆 Client/Share/Market ID 完成关联定位。
- 高频操作不依赖弹窗教程。

## 17. 发布与回退

- 每一阶段独立提交、独立验收，不以一次大规模重写发布。
- 状态契约采用新增字段，旧字段暂时保留。
- 焦点联动先完成无副作用的视觉高亮，再接入自动滚动和 URL 状态。
- 地图高流量聚合先使用真实回放数据验证，再替换现有逐请求表现。
- 任何阶段出现误判时，优先回退新的状态展示，不改变实际调度逻辑。
- 删除兼容回退逻辑前，确认所有部署均返回统一状态字段。

## 18. Definition of Done

一项功能只有同时满足以下条件才算完成：

- 状态和原因使用统一数据契约。
- 中英文文案完整。
- 支持鼠标和键盘。
- 轮询刷新不破坏焦点和上下文。
- 有单元或集成测试覆盖关键规则。
- 不记录或暴露敏感信息。
- 在 1280、1440、1920 桌面宽度下验证。
- `npm run typecheck` 和 `npm run build` 通过。
- 涉及 Rust 数据结构时，相关 Rust 测试通过。
- 完成一次基于真实异常场景的人工任务验证。

## 19. 建议执行顺序

```text
状态语义与测试夹具
→ 后端统一状态契约
→ 前端统一状态展示
→ Dashboard 焦点模型
→ Client/Share/Market 联动
→ 地图请求联动与聚合
→ 诊断抽屉
→ 操作恢复验证
→ 本地观测与用户任务测试
```

该顺序先解决“系统说什么”，再解决“用户如何从线索走到结论”，最后优化熟练使用和学习过程，避免在状态语义不稳定时提前构建复杂联动。
