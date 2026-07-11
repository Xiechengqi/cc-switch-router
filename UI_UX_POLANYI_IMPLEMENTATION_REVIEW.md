# Router UI/UX 默会知识方案实施 Review

日期：2026-07-11

对应计划：`UI_UX_POLANYI_IMPLEMENTATION_PLAN.md`
结论：工程实施完成，无已知阻断问题

## 1. 实施结果

### 统一状态语义

- Router 后端统一生成 Client、Share、Market 的 `OperationalSummary`。
- 状态包含主结论、主要原因、严重级别、发生时间、当前值和阈值。
- 前端所有列表、卡片和抽屉优先使用同一份后端结论。
- 前端保留旧响应回退逻辑，避免新旧 Router 响应切换时界面失效。
- 主动 Disabled/Maintenance 与故障 Offline/Degraded 分离。
- 停用 Share 不会使 Client 误降级。

### From-to 焦点链路

- 新增 Dashboard 全局焦点模型。
- Request、Client、Share、Market 任一时刻只有一个主焦点。
- 地图节点、请求 ticker、Client、Share 和 Market 可以双向定位。
- 无关实体降低视觉权重但不被隐藏，保留整体系统上下文。
- 焦点条明确显示当前对象及关联实体数量。
- `Esc`、焦点条按钮可以退出焦点。
- 焦点和当前详情抽屉写入 URL query，刷新和复制链接后可恢复。

### 地图

- 地图位置、默认展开、高度和导航栏保持不变。
- 请求流根据实时请求量调整亮度、宽度、动画和聚合计数。
- 成功、失败和高延迟请求使用不同颜色与残留强度。
- 高流量路径停止逐请求强闪烁，改用路径强度和计数。
- 地图节点位置不因焦点变化而改变。
- 新增请求流和需求热度图层开关，并保存本地偏好。
- 地图 Request ticker 和 Client 节点支持键盘聚焦。

### Client、Share、Market

- 状态变化立即改变视觉，但异常优先排序经过稳定窗口后才调整。
- 存在焦点时冻结默认异常排序，避免用户操作中实体换位。
- Client 展示统一主原因，并保留收款摘要和横向 Share 结构。
- Share 卡片只展示统一状态模型中的首要问题和额外问题数量。
- Market 使用同一状态模型，不再出现 Status 与 Health 原因矛盾。
- Market Activity 使用真实近期请求数据生成轻量趋势；样本不足时不渲染。
- 排序、状态筛选、地区筛选、仅异常和地图图层偏好保存到 local storage。
- 搜索内容不保存，避免把地址、Owner、URL 等输入写入浏览器持久状态。

### 诊断与操作验证

- Client、Share、Market 抽屉首先展示运行诊断。
- 诊断顺序为状态、主要原因、持续时间、影响和支撑证据。
- 支撑证据可以直接定位到健康时间线或相关详情。
- 配置操作区分“API 提交成功”和“后续 Dashboard 已观察到运行状态”。
- Share 变更待应用、被拒绝、路由恢复分别反馈。
- Market 维护状态可以按预期状态进行后续确认。
- 运行状态在确认窗口内未出现时显示“尚未确认”，不误报恢复。

### 本地隐私观测

- 新增本地 `dashboard_ux_events` 表和写入接口。
- 默认关闭：`CC_SWITCH_ROUTER_UX_TELEMETRY_ENABLED=false`。
- 可配置保留天数，范围被限制在安全区间。
- 数据最多保留 10,000 条。
- 事件类型、来源和目标类型均使用服务端白名单。
- 不接受或保存实体 ID、邮箱、URL、地址、Token、请求正文和搜索内容。
- elapsed、step count 等数值在服务端限制上限。
- Settings 中提供开关与保留时间配置。

## 2. Review 中发现并修正的问题

1. Client 全部 Share 主动停用时可能被误判离线：改为使用 tunnel/heartbeat，且忽略停用 Share 的健康失败。
2. Client 搜索只展示部分 Share 时可能按可见子集计算状态：改为始终使用完整 Share 集合。
3. Market 容量告警与 Health 文案可能矛盾：改为使用后端统一原因。
4. 状态阈值的 `startedAt` 曾使用每次快照时间：无法确认真实起点的阈值不再伪造时间；过期、编辑、离线等使用稳定来源。
5. 操作确认曾依赖浏览器与服务器时钟比较：改为比较 Dashboard snapshot 标识，避免时钟偏差。
6. UX telemetry 的 source 最初允许任意文本：改为服务端白名单，防止借字段写入敏感信息。
7. 高频聚焦可能随 5 秒轮询重复记录 telemetry：增加焦点去重。
8. Share 卡片待应用/应用失败状态在操作收纳后可能丢失：保留为主原因并控制编辑按钮状态。
9. 地图静态导出无法验证真实 API：使用隔离临时 Router 和临时 SQLite 完成浏览器运行验证，随后清理全部临时文件。

## 3. 验证结果

### 自动测试

- `cargo test`：213 passed，0 failed。
- 新增覆盖：离线 Share/Client 聚合状态、主动暂停 Share、Market 满容量和本地隐私事件。
- `npm run typecheck`：通过。
- `npm run build`：通过。
- Next.js `/`、`/metrics`、`/settings` 全部完成静态生成。
- `cargo fmt --check`：通过。
- `git diff --check`：通过。

### 浏览器验证

通过隔离的临时 Router 实例验证：

- 1440×1200：地图、图层开关、Client/Market 工具栏、表格和空状态正常。
- 1280×1000：无页面级横向溢出，工具栏未重叠，Market 八列结构可用。
- 地图保持默认展开和原有主要位置。
- 临时数据库、Metrics 数据库、SSH key、截图和服务均已删除或停止。

## 4. 数据与安全 Review

- Dashboard 新字段为增量字段，未删除原始状态字段。
- 未修改请求调度、限流、Market 优先级或代理路径。
- 未修改支付、收款信息或链上逻辑。
- UX telemetry 默认不启用，不向第三方发送数据。
- telemetry 写入接口即使启用也只接受有限枚举和数值。
- URL 恢复仅写入实体类型和内部 ID，不写入邮箱、地址或 Token。
- local storage 不保存搜索词和焦点实体内容。

## 5. 持续观测

真实用户的默会知识无法由自动测试代替。工程已提供完成情境观察、关键事件访谈和专家/新手任务比较所需的焦点链路与隐私观测能力。部署环境如需采集，应由管理员主动启用本地 UX telemetry，并在约定周期后依据本地聚合数据调整阈值或提示；该持续观测不阻塞本次工程交付。
