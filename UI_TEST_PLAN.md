# Router UI 功能测试计划

手动 UI 回归清单。目标是**每个用户可触达的控件都有对应用例**,并且在代码改动后能快速定位需要重跑的用例。

- 界面实现见 [ARCHITECTURE.md](ARCHITECTURE.md) 第 9 节
- 后端接口契约见 [PROTOCOL.md](PROTOCOL.md)

---

## 0. 维护约定

**本文件与代码同步修改。** 改动前端时:

1. 新增/删除控件 → 在对应用例表里增删行
2. 新增组件文件 → 在 §18 反查索引里登记
3. 新增 `localStorage` 键 → 加入 §3 清理清单
4. 新增 API 调用 → 在 §19 覆盖核对表里登记调用它的用例

**判断要不要更新本文件的最快方法**:`grep` 你改动的文件名,§18 会告诉你哪些用例受影响。

现状:仓库**没有前端自动化测试**(无 jest/vitest/playwright),后端有 Rust 内联测试和静态契约审计。浏览器交互仍以这份手动清单为验收依据。

---

## 1. 测试环境

### 1.1 后端

```bash
# debug 构建默认开启鉴权旁路,自动登录为 dev-admin@localhost
cargo run

# 要测试匿名/权限行为,必须显式关闭
CC_SWITCH_ROUTER_DEV_AUTH_BYPASS=0 cargo run
```

> **务必注意**:`dev_auth_bypass_enabled()` 在 `#[cfg(debug_assertions)]` 下**未设置即为 true**(`src/api.rs:4514`)。也就是说 debug 构建默认是登录态。所有"未登录应该看到什么"的用例,不关掉旁路就是无效测试。

### 1.2 前端

```bash
cd frontend && npm run dev     # /v1/* 代理到 CC_SWITCH_ROUTER_DEV_API_TARGET(默认 127.0.0.1:8787)
```

### 1.3 角色切换

| 角色 | 怎么获得 |
|---|---|
| 匿名访客 | `DEV_AUTH_BYPASS=0` 且不登录 |
| 普通登录用户 | 邮箱验证码登录,或 `CC_SWITCH_ROUTER_DEV_AUTH_EMAIL=user@test` |
| 供给方 Host Provider | 该邮箱名下登记过主机 |
| 租客 Renter | 该邮箱租用中有 Client |
| 管理员 Admin | 邮箱在 `CC_SWITCH_ROUTER_ADMIN_EMAILS` 中 |
| 官方供给方 | 邮箱 = `CC_SWITCH_ROUTER_OWNER_EMAIL` |

**角色可叠加。** 同一用户既是供给方又是租客是常见情况,`is_host_owner` / `is_client_owner` 是**逐主机行**计算的,不是用户属性。用例 H-30 专门覆盖这个叠加场景。

---

## 2. 角色 × 界面 可达矩阵

测试时按角色分轮次跑,避免反复切换环境。

| 界面 | 匿名 | 普通 | 供给方 | 租客 | 管理员 |
|---|:--:|:--:|:--:|:--:|:--:|
| `/clients` 总览 + 地图 | ✅ 只读 | ✅ | ✅ | ✅ | ✅ |
| `/markets` | ✅ 只读 | ✅ | ✅ | ✅ | ✅ 可编辑 |
| `/share-market` | ✅ 只读 catalog | ✅ 租用/挂售 | ✅ | ✅ | ✅ 无特权 |
| `/account/share` | ❌ | ✅ 只读监控 | ✅ | ✅ | ✅ 无特权 |
| `/account/billing` | ❌ | ✅ 应付/策略 | ✅ 应收/策略 | ✅ 应付 | ✅ 含争议裁决 |
| `/client-market` 主机表 | ✅ 只读 | ✅ 空表 | ✅ 全功能 | ✅ 空表 | ✅ 无特权 |
| `/rentals` | ❌ 提示登录 | ✅ 空 | ✅ 空 | ✅ 有数据 | ✅ |
| `/account` | ❌ | ✅ | ✅ | ✅ | ✅ |
| `/settings` | ❌ | 部分只读 | 部分只读 | 部分只读 | ✅ 全功能 |
| `/metrics` | ❌ | ❌ 提示 | ❌ 提示 | ❌ 提示 | ✅ |
| Share 页(子域名) | ✅ 只读 | ✅ | ✅ | ✅ | ✅ |

> **管理员在 Client Market / Share Market 无特权**。主机操作只认 host owner；拼车位操作只认 Share owner / 租客本人。用例 H-31、SM-08 覆盖。

---

## 3. 每轮测试前的状态清理

浏览器持久化状态是手动测试最常见的假阳性来源。**换角色或跑筛选类用例前,先清空**:

```js
// DevTools Console
Object.keys(localStorage).filter(k => k.startsWith('cc-switch') || k.startsWith('cc_switch')).forEach(k => localStorage.removeItem(k));
sessionStorage.removeItem('cc_switch_router_web_terminal_windows_v1');
location.reload();
```

当前共 30 个持久化键。按域分组:

| 域 | 键 |
|---|---|
| 认证 | `cc_switch_router_auth_v1`, `cc-switch-router-auth-refresh-v1` |
| 语言 | `cc_switch_router_locale_v1` |
| 公告 | `..._announcement_dismiss_today_v1`, `..._dismiss_permanent_v1` |
| Clients 页 | `..._client_status_v1`, `..._client_sort_v1`, `..._client_expanded_v2`, `..._client_regions_v2`, `..._client_region_v1` |
| Markets 页 | `..._market_status_v2`, `..._market_sort_v1` |
| Client Market | `..._owner_scope_v2`, `..._status_filter_v2`, `..._sort_v2`, `..._region_filter_v1`, `..._payment_filter_v1` |
| 添加主机 | `cc-switch.client-market.add-host.mode`, `...ssh-key-open` |
| 新建 Client | `..._create_client_providers_v2`, `..._create_client_regions_v2` |
| 窗口 | `..._console_windows_v1/v2`, `..._web_terminal_windows_v1`(sessionStorage) |
| Share 页 | `cc_switch_share_api_email_v1`, `cc_switch_share_api_token_v1` |
| 其他 | `..._chat_anon_visits_v1`, `..._map_request_ticker_expanded_v1`, `..._board_guest_v1`, `cc-switch-router-client-upgrade-state` |

> **注意版本号后缀**。`owner_scope_v2` / `sort_v2` / `status_filter_v2` 是近期升版的键;老用户升级后会被重置一次,用例 H-01 覆盖这个首次进入行为。

---

## 4. 认证与外壳(A)

覆盖 `components/layout/app-shell.tsx`、`components/auth/*`、`components/announcement/*`

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| A-01 | 匿名 | 打开任意页 | 右上显示「登录」按钮;加载中时按钮 disabled |
| A-02 | 匿名 | 点登录 → 输邮箱 → 发送验证码 | 进入 6 位验证码步骤 |
| A-03 | 承 A-02 | 输入错误验证码 | 红色错误 Alert,不关闭弹窗 |
| A-04 | 承 A-02 | 输入正确验证码 | 自动提交(填满 6 位即提交),弹窗关闭,右上变头像 |
| A-05 | 承 A-02 | 点「重新发送」 | 验证码框清空,按钮短暂 disabled |
| A-06 | 承 A-02 | 点「换个邮箱」 | 回到邮箱步骤 |
| A-07 | 已登录 | 点头像 | 菜单显示邮箱(disabled)、API Token、退出 |
| A-08 | 管理员 | 点头像 | 菜单**额外**显示 Metrics、Settings,均新标签页打开 |
| A-09 | 普通用户 | 点头像 | 菜单**不含** Metrics / Settings |
| A-10 | 已登录 | 菜单 → API Token | 弹窗显示前缀;眼睛图标切换明文;复制按钮显示「已复制」1.5 秒 |
| A-11 | 承 A-10 | 点「重置并显示」 | 生成新 token 并明文展示 |
| A-12 | 已登录 | 菜单 → 退出 | 回到未登录态,localStorage 认证键清空 |
| A-13 | 任意 | 切换 EN / 中文 | 全站文案即时切换,刷新后保持 |
| A-14 | 配置了多区域 | 点区域下拉 | 列出 `/v1/regions` 的区域;选择后跳转到该区域域名 |
| A-15 | 窄屏 < 640px | 观察顶栏 | 区域切换器隐藏(`hidden sm:flex`),导航 tab 只剩图标 |
| A-16 | 公告已启用且未忽略 | 打开首页 | 弹出公告 |
| A-17 | 承 A-16 | 点「今日不再提示」 | 关闭;当天刷新不再弹;次日再弹 |
| A-18 | 承 A-16 | 点「不再提示」 | 关闭;刷新不再弹 |
| A-19 | 承 A-16 | 点弹窗 X | 关闭但**不写忽略状态**,刷新后仍弹 |
| A-20 | 任意 | 逐个点 5 个导航 tab | 分别到 clients / markets / client-market / rentals / account,选中态正确 |
| A-21 | 清空 `cc_switch_router_auth_v1`;拦截 Network | 同一标签页同时触发 AuthProvider 初始化和发送验证码 | `/v1/installations/register` 只发送 1 次;后续 request-code 与 localStorage 使用同一 `installationId` |
| A-22 | 清空认证 localStorage;浏览器支持 Web Locks | 同时打开两个 Dashboard 标签页 | 两页最终持有相同完整 installation 身份;总计只注册 1 个 installation;任一页发码后均不覆盖身份 |
| A-23 | 承 A-02;拦截 verify-code | 快速输入或粘贴完整 6 位验证码 | 只发送 1 次 verify-code;body 使用最新 6 位值和 request-code 时的 `installationId` |
| A-24 | 两个独立浏览器 profile/设备,同一邮箱 | 设备 A 请求验证码;设备 B 随后请求验证码;分别输入各自邮件中的验证码 | 两个 challenge 均有效;设备 B 的请求不使设备 A 报 `expired or not found` |
| A-25 | 同一设备和邮箱 | 请求验证码后点「重新发送」;先输入旧码再输入新码 | 旧码失败且新码成功;其他设备的有效验证码不受影响 |
| A-26 | 清空认证状态;拦截 request-code | 快速双击发送或在同一浏览器两个标签页同时为同一邮箱发码 | 前端单标签页只发 1 次;跨标签页后到请求命中冷却且不再发送第二封邮件 |

---

## 5. Clients 总览(C)

覆盖 `clients-page.tsx`、`live-map.tsx`、`client-board.tsx`、`share-card.tsx`、`drawer-panels.tsx`、`presence-footer.tsx`

轮询:`getDashboard()` 每 **5 秒**;presence 每 **15 秒**;账单每 **20 秒**。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| C-01 | 有数据 | 打开 `/clients` | 地图 + Client 列表渲染;页脚显示 PAGE ONLINE |
| C-02 | 无数据 | 同上 | 地图显示 waiting/no data 占位 |
| C-03 | 有多国数据 | 点地图上某国圆点 | 下方列表按该国筛选 |
| C-04 | 同上 | 悬停国家形状 | 显示该国 client/share 数量 tooltip |
| C-05 | 有请求流 | 点活动流展开/收起 | 面板展开;状态持久化到 `..._map_request_ticker_expanded_v1` |
| C-06 | 同上 | 悬停活动流 | 解锁完整滚动(最多 100 条),移开恢复 5 行 |
| C-07 | 有 client | 点状态筛选 5 个 tab | 列表按 全部/在线/重连/降级/离线 过滤;持久化 |
| C-08 | 有 client | 搜索框输入 ID/邮箱/区域/子域名/IP | 命中项保留;清空恢复 |
| C-09 | >1 个区域 | 使用区域多选 | 按国家码过滤;仅在多区域时出现该控件 |
| C-10 | 有 client | 切换排序(问题/名称/最近/运行/Token/Share) | 顺序变化;持久化 |
| C-11 | 筛选后无结果 | 观察空态 | 显示「清除筛选」链接,点击后恢复全量 |
| C-12 | 有 client | **单击**卡片头 | 展开/收起;展开集合持久化 |
| C-13 | 有 client | **双击**卡片头 | 打开详情抽屉 |
| C-14 | client 有 tunnel | 点外链图标 | 新标签打开 tunnel URL |
| C-15 | client 有 tunnel | 点 Console | 打开内嵌 iframe 控制台面板 |
| C-16 | `chatAvailable` | 点 Chat | 打开聊天面板;有未读时显示角标 |
| C-17 | 有 share | 点 share 卡片体 | 打开 share 详情抽屉 |
| C-18 | share 非 disabled | 点 Connect | 打开连接弹窗(见 §11) |
| C-19 | 有 share | 点 Edit / View | `canManage` 时可编辑,否则只读视图 |
| C-20 | 有待应用编辑 | 观察 share 卡片 | 显示「待应用」;被拒绝时显示错误 tooltip |
| C-21 | 有可升级 client | 点升级按钮 | 弹二次确认后开始升级 |
| C-22 | 任意 | 保持页面 30 秒 | 数据自动刷新,展开态/筛选态不被重置 |
| C-23 | 配置了 Telegram | 观察页脚 | 显示 Telegram 链接,新标签打开 |

---

## 6. Markets(M)

覆盖 `markets-table.tsx`(含内联 `MarketEditDialog`、`MarketSharePriorityPanel`)

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| M-01 | 有 market | 打开 `/markets` | 表格渲染 |
| M-02 | 有 market | 点 4 个状态 tab | 按 全部/可用/异常/停用 过滤;持久化 |
| M-03 | 有 market | 搜索 ID/名称/邮箱/子域名/URL | 命中过滤 |
| M-04 | 有 market | 切换排序(问题/名称/容量/活跃/Share/更新) | 顺序变化;持久化 |
| M-05 | 有 market | 点行 / 键盘 Enter / Space | 打开详情抽屉(键盘可达) |
| M-06 | 有 market | 点行内外链 | 新标签打开 `publicBaseUrl` |
| M-07 | 筛选无结果 | 观察 | 「清除筛选」链接可用 |
| M-08 | 任意 | 点「安装 Market」 | 打开安装指引弹窗,含 GitHub releases 链接 |
| M-09 | 抽屉内 | 切换 Claude/Codex/Gemini 优先级 tab | 拉取并展示对应 app 的 share 优先级 |
| M-10 | `canManage` | 点 Edit | 打开编辑弹窗 |
| M-11 | 承 M-10 | 勾选维护模式 + 填消息 → 保存 | 保存成功;消息框在未开启维护时 disabled |
| M-12 | 承 M-10 | 勾选若干 share → 停用所选 | 对应 share 进入停用集合 |
| M-13 | 承 M-10 | 点 全部启用 / 全部停用 | 批量生效 |
| M-14 | 有阻塞状态 | 点单条 Release / 全部 Release | 逐条释放 |
| M-15 | 非 `canManage` | 打开抽屉 | 无 Edit 按钮 |

---

## 6.1 Share Market 拼车位(SM)

覆盖 `share-market-page.tsx`。轮询 5 秒；弹窗、确认框或操作进行中时暂停刷新。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| SM-01 | 任意 | 打开 `/share-market` | 顶部顺序为 Token Market / Share Market / Client Market；空 catalog 正常显示 |
| SM-02 | 未登录 | 点租用或添加 Share | 打开登录弹窗,不提交写请求 |
| SM-03 | 已登录且拥有 active Share | 点添加 Share | 只列出未挂售 Share；可一次添加 1-20 个拼车位 |
| SM-04 | 添加拼车位 | 保持价格模式为免费 | 默认固定期限 1 天；请求中日费率和币种均为空，`freeDurationDays=1` |
| SM-05 | 添加付费拼车位 | 输入三位以上小数、非 CNY/USD 币种,或未配置对应币种收款资料/付款宽限 | 内联报错或阻止发布,不提交 |
| SM-06 | 有可用拼车位 | 可信买家租用 | 座位进入 pending/occupied；同一用户不能重复租同一 Share；已有 direct grant 的用户不显示租用按钮 |
| SM-07 | 付费租约 | 查看商品与租约 | 只显示收款方式种类和联系方式；不显示账号、地址、二维码或单商品付款按钮 |
| SM-08 | owner 查看已租座位 | 强制回收 / 回收并拒绝后续访问 | 二次确认后进入回收状态；拒绝后该买家不能新租该 owner 的 Share 座位 |
| SM-09 | owner 打开市场准入 | 将该买家的 Share 规则改回允许 | 保存后买家可再次新租 Share；Client Host 规则不受影响 |
| SM-10 | listing 有活跃租约 | 停止挂售 | 空闲座位关闭,活跃租约继续显示且可正常使用；「添加 Share」仍不可选该 Share |
| SM-11 | 已释放的座位 | 删除座位 | 座位从 catalog 消失,历史订阅和账单仍保留 |
| SM-12 | 窄屏 | 检查导航、弹窗和座位表 | 导航和表格可横向滚动；弹窗纵向滚动；文字和操作不重叠 |
| SM-13 | 停止挂售且无活跃租约 | 点添加 Share | 该 Share 重新出现在候选列表；可新建 listing |
| SM-14 | 离线 Share 的可用座位 | 观察并直接调用租用接口 | 已登录用户不显示租用按钮；直接请求返回离线冲突,不创建订阅或账务合约 |
| SM-15 | 租约非终态 | My rentals / owner 嵌套订阅 | 有 subdomain 时显示「打开 Share」并可跳转 |
| SM-16 | 已登录买家不在 owner 白名单,Share 在线且座位可用 | 点「租用」 | 保留租用按钮；弹出中性授权引导而非红色英文错误，明确只有白名单用户可租用并显示当前登录邮箱；点主操作打开该 Share 对应的 Client 聊天室 |
| SM-17 | 新建/编辑免费拼车位 | 分别输入 1、365、0、366 天并切换永久 | 1/365 可保存；0/366 前后端拒绝；永久请求省略 `freeDurationDays` |
| SM-18 | 付费拼车位 | 直接构造带 `freeDurationDays` 的请求 | 后端拒绝，不创建或修改报价 |
| SM-19 | 免费租约已生效 | 查看 All / Mine / My rentals / Account Share | 显示合同冻结期限、激活与到期时间；固定期限显示剩余时间，永久明确标注永久 |

### 6.1.1 Share Market ↔ Server 联调(SM-E2E)

需 Router + 至少一台在线 Server。关闭 `DEV_AUTH_BYPASS` 或使用两套邮箱分别作为 owner / renter。

| ID | 步骤 | 预期 |
|---|---|---|
| SM-E2E-01 | owner 添加 Share 并挂出免费拼车位 | listing 出现在 All；Server 侧尚无新 grant |
| SM-E2E-02 | renter 租用 | Router 订阅 `grant_pending` → pending edit → Server ack 后出现 `routerShareMarket` shareto |
| SM-E2E-03 | renter 打开 Share 并鉴权调用 | 请求成功；用量计入该 grant 限额 |
| SM-E2E-04 | owner 强制回收 | revoke pending edit → grant 移除 → renter 再请求被拒；座位回到可租 |
| SM-E2E-05 | 付费座位完成 grant | 订阅进入 `active_postpaid`；前 12 小时健康服务时长不累计费用 |
| SM-E2E-06 | 超过试用且保持在线 | 按健康秒数累计到 owner+renter+币种的赊账账户；不生成单商品账单 |
| SM-E2E-07 | 租用 1 天免费座位，Server 延迟应用 grant | 到期时间从 edit-ack 实际生效时开始，而非点击租用时开始；到期前 24 小时事件只产生一次 |
| SM-E2E-08 | 免费期限到期 | 自动进入 revoke；Server ack 后座位恢复可租；revoke 失败时保持失败/回收态且不会提前释放座位 |
| SM-E2E-07 | 同一 renter 租用该 owner 的多个 Share/Client Host 并达到阈值 | 生成一张含多个服务明细的聚合账单并暂停相关服务；完整收款资料只在该账单中出现 |

---

## 7. Client Market 主机表(H)

覆盖 `client-market-page.tsx` 与 `client-market/*`。**本区域近期重构过,用例需与代码同步核对。**

轮询 20 秒;弹窗/批量/行内忙时暂停。

### 7.1 归属范围与默认态

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| H-01 | 清空 localStorage,已登录 | 打开 `/client-market` | **默认只显示自己的主机**(scope=mine);排序默认按状态严重度 |
| H-02 | 承 H-01,自己无主机 | 观察空态 | 提示「还没有登记主机」+「浏览全部供给」链接 |
| H-03 | 承 H-02 | 点「浏览全部供给」 | 切到全部供给,表格显示他人主机 |
| H-04 | 已登录 | 点工具栏「查看全部供给」 | 切换到全部;按钮文案变为「只看我的主机」 |
| H-05 | 承 H-04 | 刷新页面 | 保持在「全部供给」(scope 持久化) |
| H-06 | 匿名(需关旁路) | 打开 `/client-market` | 显示全部供给(mine 对匿名降级为全部);无「查看全部」按钮 |
| H-07 | 任意 | 用表头 Owner 多选筛选 | scope 转为 custom;工具栏按钮文案随之变化 |

### 7.2 筛选、排序、分页

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| H-10 | 有多状态主机 | 点 4 个状态 tab | 全部/空闲/使用中/需关注 分组正确;`需关注` tab 未选中时为琥珀色 |
| H-11 | 有主机 | 观察默认排序 | 顺序为 unreachable → abnormal → draining → locked → reserved → disabled → allocated → idle |
| H-12 | 有主机 | 连点某列表头 3 次 | 升序 → 降序 → 清除排序(回到 owner→ip→id 稳定序) |
| H-13 | 有主机 | 逐列验证排序 | status / region / owner / offer / subdomain / ip 六列均可排 |
| H-14 | 有多区域 | 用表头 Region 多选 | 按国家码过滤;持久化 |
| H-15 | 有多种收款方式 | 用表头 Payment 多选 | 按 alipay/wechat/binance/crypto/custom 过滤 |
| H-16 | >10 台主机 | 使用分页 | 上一页/下一页/页码按钮;首尾页对应按钮 disabled |
| H-17 | 承 H-16 | 改变任一筛选或排序 | 页码重置为 1 |
| H-18 | 筛选无结果 | 点「清除筛选」 | 状态/区域/支付筛选清空,**scope 回到 mine** |

### 7.3 添加主机

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| H-20 | 匿名 | 点「添加主机」 | 触发登录弹窗;登录后**自动重新打开**添加弹窗 |
| H-21 | 已登录 | 打开添加弹窗 | 双 tab:密码 / 手动密钥;记住上次选择 |
| H-22 | 密码模式 | 填 IP + root 密码 → 测试连接 | 成功/失败以 toast 或内联错误反馈 |
| H-23 | 密码模式 | 提交 | 4 步进度:安装密钥 → 连通性 → IP 情报 → 注册 |
| H-24 | 手动模式 | 展开 SSH Key 折叠区 | 显示 `authorized_keys` 安装命令 + 复制按钮;展开态持久化 |
| H-25 | 手动模式 | 提交 | **跳过安装密钥步**,3 步进度 |
| H-26 | 未配置收款资料 | 填写付费报价 | 琥珀色提示 + 跳转账户页链接;阻止提交 |
| H-27 | 任一步失败 | 点「返回」 | 回到表单,步骤状态重置,可修改后重试 |
| H-28 | 成功后 | 观察 | 成功态标题;关闭后表单重置,密码字段清空 |
| H-29 | 免费报价 | 分别输入 1、365、0、366 天并切换永久 | 1/365 可提交；0/366 前后端拒绝；永久请求省略 `freeDurationDays` |

### 7.4 角色与权限

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| H-30 | **既出租又租用**的账号 | 打开主机表 | 自有主机有完整操作;租用的主机行无供给方操作 |
| H-31 | 管理员(非主机属主) | 打开他人主机行 | **无** Edit Offer / Cleanup / Delete / 终端(管理员不被提升) |
| H-32 | 匿名 | 观察他人主机行 | 可见 IP、区域、报价、状态、子域名;**不可见**端口、指纹、备注、最后错误 |

### 7.5 行内操作

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| H-40 | 主机 `idle` 且是属主 | 观察行 | 显示「新建 Client」按钮 |
| H-41 | 非 idle | 观察行 | 无「新建 Client」按钮 |
| H-42 | 属主 | 打开操作菜单 | 按状态显示可用项(见下表) |
| H-43 | 属主 | Edit Offer → 改价 → 保存 | 报价更新;未配置收款时付费报价被阻止 |
| H-44 | 状态 unreachable/disabled/abnormal | Reverify | 执行重新校验 |
| H-45 | 有 installation 且 allocated/unreachable/draining | Cleanup | 二次确认(danger)→ 进度弹窗显示阶段与日志 |
| H-46 | 状态 unreachable/draining | 观察 Cleanup 文案 | 显示为「重试清理」 |
| H-47 | allocated 且非自己租用 | Cleanup 并勾选拒绝后续访问 | 普通 Provider 可同时把该买家的 Client Host 产品规则设为拒绝；不撤销整段关系,也不写入欠款事实 |
| H-48 | 无 installation 且 idle/disabled/abnormal | Delete | 二次确认(danger)后删除 |
| H-49 | 清理进行中 | 尝试关闭进度弹窗 | 关闭按钮 disabled;任务成功或失败后才可关 |
| H-50 | 有备注的主机 | 观察 | 备注以子行展示;**无备注则不出现空子行** |
| H-51 | 已登录买家不在 Host owner 白名单,主机 idle | 点「新建」 | 弹出中性授权引导而非红色英文错误，明确只有白名单用户可使用；显示 owner 邮箱及其 WeChat/Telegram/自定义联系方式，邮件按钮打开 `mailto:`；quote 或 commit 阶段撤销准入也回到同一弹窗 |
| H-52 | 免费 Host 获取 Quote 后 Owner 修改期限 | 提交原 Quote | Client 使用 Quote 冻结的旧期限；Host 新期限只影响后续 Quote，`offerRevision` 已递增 |
| H-53 | 免费 Client provisioning 成功 | 查看租约与 Account Client | 激活/到期从开通成功时计算并展示；永久租约无到期时间 |
| H-54 | 免费 Client 到期 | 等待 20 秒对账 | 产生一次临期/到期事件并启动 `free_period_expired` cleanup；失败时为 `release_failed` 且 Host 不回 idle，重试成功后才释放 |

**操作可用性矩阵**(行=主机状态,列=菜单项;均需 host owner):

| 状态 | Edit Offer | Reverify | Cleanup | Delete |
|---|:--:|:--:|:--:|:--:|
| idle | ✅ | — | — | ✅ |
| allocated | ✅ | — | ✅ | — |
| draining | ✅ | — | ✅(重试) | — |
| unreachable | ✅ | ✅ | ✅(重试) | — |
| abnormal | ✅ | ✅ | — | ✅ |
| disabled | ✅ | ✅ | — | ✅ |

### 7.6 选择模式与批量

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| H-60 | 有主机 | 点「选择」 | 进入选择模式,出现复选框列与批量工具栏 |
| H-61 | 选择模式 | 点表头复选框 | 三态:全选本页 / 部分为不确定态 / 再点取消本页 |
| H-62 | 选择模式 | 点「全选筛选结果」 | 跨页选中所有可见主机 |
| H-63 | 选择模式 | 点「本页」/「清除」 | 相应增减选择 |
| H-64 | 选中若干 | 观察 4 个批量按钮 | 各显示「可执行 N / 已选 M」;可执行为 0 时 disabled |
| H-65 | 选中含不可清理项 | 批量清理 | 确认框显示将执行数与跳过数 |
| H-66 | 承 H-65 | 确认 | 进度弹窗逐条显示 排队/执行中/成功/失败/跳过 |
| H-67 | 批量部分失败 | 结束后观察 | toast 汇总;**保持选择模式且只选中失败项**便于重试 |
| H-68 | 批量全部成功 | 结束后观察 | 退出选择模式,选择清空 |
| H-69 | 批量进行中 | 尝试操作 | 所有批量按钮与复选框 disabled;后台轮询暂停 |
| H-70 | 选择模式 | 退出登录 | 自动退出选择模式并清空选择 |
| H-71 | 选中后改筛选 | 观察 | 离开筛选范围的选择被自动丢弃 |

### 7.7 导入导出

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| H-80 | 有自有主机 | 点导出 | 弹窗显示行格式文本 + 复制按钮 |
| H-81 | 选择模式选中若干 | 点「导出所选」 | 仅导出可导出项;完成后清空选择 |
| H-82 | 任意 | 点导入 → 粘贴合法文本 → 提交 | 显示导入结果:成功/跳过/失败计数与逐条明细 |
| H-83 | 导入非法行 | 提交 | 指出出错行,不整体失败 |
| H-84 | 导入超 1MB | 提交 | 拒绝并提示 |

---

## 8. 我的租用(R)

覆盖 `client-market/rentals-page.tsx`、`my-rentals-panel.tsx`、`client-market-rental-banner.tsx`、`client-market/release-rental-action.tsx`

### 8.1 租用列表

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| R-01 | 未登录 | 打开 `/rentals` | 提示登录,不发数据请求 |
| R-02 | 登录但无租用 | 打开 | 虚线空态卡片 |
| R-03 | 有租用 | 打开 | 每条租用一张卡:国旗、子域名、主机状态、供给方邮箱与释放入口 |
| R-04 | **既出租又租用** | 打开 | **只显示自己租的**,自己出租给别人的不出现 |
| R-05 | 付费租用 | 观察 | 只显示跳转统一 Market Billing 的入口；无单商品金额、付款倒计时或付款声明按钮 |
| R-06 | 承 R-05 | 点 Market Billing | 跳转 `/account/billing/`；尚未出账时只显示供应商赊账账户，不显示完整收款账号/二维码 |
| R-07 | 已生成聚合账单 | 在 Account Billing 打开当前账单 | 显示冻结的收款方式、二维码、总金额、截止时间及多个服务明细 |
| R-11 | 供给方中途改价 | 刷新租用与账务 | 既有服务合约仍按租用时快照费率计费；新报价只影响后续租用 |
| R-12 | 任意 | 保持 20 秒 | 自动刷新 `/v1/client-market/my-rentals`；不请求任何旧 Client 商品账单接口 |

### 8.2 释放(任何状态均可发起)

释放入口常驻卡片,**不依赖是否已出聚合账单**。释放只终止后续计费并清理 Client；已产生但尚未出账的余额会进入最终聚合账单。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| R-20 | 付费租用仍处于试用或累计阶段 | 打开 `/rentals` | 卡片上有释放入口 |
| R-21 | **免费租用**(无价格) | 打开 | 同样有释放入口 |
| R-22 | 已存在聚合账单 | 打开 | 释放入口与 Market Billing 跳转并存；释放不改写已生成账单 |
| R-23 | 任一可释放状态 | 点释放 | 二次确认(danger),明确隧道立即停用、远程安装被清理且数据可能永久丢失 |
| R-24 | 承 R-23 | 点取消 | 无任何请求发出 |
| R-25 | 承 R-23 | 点确认 | 进入**清理进度弹窗**,显示阶段 chip + 实时作业日志 |
| R-26 | 承 R-25 | 观察阶段推进 | 依次经过 stop → wipe → purge,阶段 chip 随之更新 |
| R-27 | 承 R-25 | 清理进行中尝试关闭弹窗 | 关闭按钮 **disabled**,点遮罩也不关 |
| R-28 | 承 R-25 | 清理成功 | 绿色完成文案 + 成功 toast;关闭按钮可用;列表中该租用消失 |
| R-29 | 承 R-25 | 清理失败 | 红色失败指引(按 `failureCode` 给出可操作建议)+ 失败 toast;关闭按钮可用 |
| R-30 | 承 R-29 | 状态变 release_failed | 卡片显示错误文案 + 「重试释放」按钮 |
| R-31 | 承 R-30 | 点重试释放 | 与 R-23 相同的确认文案与进度弹窗 |
| R-32 | 状态 releasing | 观察 | 显示释放中;**卡片不再显示释放入口**(已有作业在跑) |
| R-33 | 清理超时(>3.6 分钟无终态) | 观察 | 超时 toast,不无限转圈 |
| R-34 | 释放进行中 | 直接刷新页面 | 不崩溃;后台作业继续,状态由轮询反映 |
| R-35 | Account Billing 中查看该服务明细 | 释放 Client | 只能返回租用页发起释放；账单对话框不提供商品生命周期操作 |

---

## 9. 账户(AC)

覆盖 `account-page.tsx`。收款资料直接影响付费报价能否发布,是 Client Market 的前置。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| AC-01 | 已登录 | 打开 `/account` | 显示收款资料；市场准入与授信由独立导航进入 `/account/market-access` |
| AC-02 | 未配置 | 观察 | 空态;Client Market 发布付费报价时会被此状态阻止(见 H-26) |
| AC-03 | 任意 | 添加支付宝 | 填账号 → 保存 → 重新加载后仍在 |
| AC-04 | 任意 | 添加微信 | 同上 |
| AC-05 | 任意 | 添加币安 | 同上 |
| AC-06 | 任意 | 添加加密货币 | 币种下拉含 USDT / USDC;链下拉含全部支持链 |
| AC-07 | 承 AC-06 | 填入**格式非法**的地址 | 按链校验拒绝(EVM 与 Tron 格式不同),给出可理解的错误 |
| AC-08 | 承 AC-06 | 同一币种+链重复添加 | 应给出明确反馈,不产生重复条目 |
| AC-09 | 任意 | 添加自定义方式 | 说明文本可填;超长(>2000 字符)被拒 |
| AC-10 | 任一方式 | 上传二维码图片 | 正常图片保存成功并可预览 |
| AC-11 | 承 AC-10 | 上传 >4MB 或超 4096px 的图 | 拒绝并提示 |
| AC-12 | 承 AC-10 | 上传非图片文件 | 拒绝 |
| AC-13 | 已配置多种 | 删除其中一种 | 该方式消失;其余保留;二维码资产同步清理 |
| AC-14 | 已配置 | 保存后立刻去 Client Market 发布付费报价 | 不再被阻止(与 H-26 呼应) |
| AC-15 | 仍有付费报价、活跃服务、余额或未结账单 | 尝试清空全部收款方式 | 后端拒绝并说明必须先清理市场账务依赖 |
| AC-16 | 已移除付费报价且所有账务依赖结清 | 清空全部收款方式 | 保存成功；免费商品和市场准入关系不受影响 |
| AC-17 | 无收款方式 | 观察 | 显示收款资料空态；市场准入关系仍在 `/account/market-access` 管理 |
| AC-18 | 修改收款资料后 | 分别查看修改前后生成的账单 | 已生成账单继续显示原冻结快照且无需重新确认；后续新账单使用新资料与新的 `paymentProfileUpdatedAt` |

### 9.1 账户 Share 只读监控(AS)

覆盖 `account-share-page.tsx`。20s 轮询；**无任何写操作按钮**(租用/付款/回收等只在 Share Market)。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| AS-01 | 已登录 | 打开 `/account/share` | 侧边栏有 Share Market / Share 市场(在 Client 之前)；默认 User tab；只读提示可见 |
| AS-02 | 有租约 | User tab | 显示订阅卡:状态、截止、报价、owner；异常态边框强调；无付款/归还按钮 |
| AS-03 | 有挂售 | Provider tab | 显示 listing 摘要(空闲/占用/需关注)与租客租约卡；无强制回收按钮 |
| AS-04 | User 卡 | 点「在 Share Market 中管理」 | 跳到 `/share-market/?tab=rentals&focus=…` |
| AS-05 | Provider 卡 | 点管理 | 跳到 `/share-market/?tab=mine&focus=…` |
| AS-06 | 有 subdomain | 点「打开 Share」 | 新标签打开 Share 子域 |
| AS-07 | 空态 | User / Provider 无数据 | 空态 + 「打开 Share Market」链接 |
| AS-08 | 未登录 | 打开 `/account/share` | 提示登录(账户区本身通常需登录) |
| AS-09 | 固定期限与永久免费租约并存 | 查看 User / Provider 卡片 | 报价分别显示天数或永久；已生效租约显示激活和到期时间，不再把全部免费租约写成永久 |

### 9.2 市场准入与授信(MA)

覆盖 `account-market-access-page.tsx`。页面按 Share / Client Host × 免费 / 付费展示四个独立作用域；免费隐式默认黑名单，付费隐式默认白名单。付费租用除准入外还必须获得信用额度。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| MA-01 | 新供应商账户 | 打开 `/account/market-access` | Share / Client Host 的免费作用域显示黑名单，付费作用域显示白名单；未知用户可租免费商品但不能租付费商品 |
| MA-02 | 买家尚未注册 | 按邮箱添加可信买家并分别设置四个作用域 | 关系保存为邮箱预授权；买家注册并首次租用时绑定其用户 ID |
| MA-03 | 可信买家无信用额度 | 分别租免费与付费商品 | 免费商品可租；付费商品拒绝并提示需供应商授信 |
| MA-04 | 供应商给买家有限额度 | 保存后再更新额度 | CNY / USD 独立保存,revision 递增；后续账户使用新额度协调状态 |
| MA-05 | 供应商选择无限额度 | 未确认/确认风险分别保存 | 未确认被前后端拒绝；确认后保存为无限且不要求金额 |
| MA-06 | 买家有免费与付费服务 | 撤销整个买家关系 | 新租全部拒绝；现有付费服务终止且历史账单付款后不恢复,现有免费服务不被策略更新直接中断 |
| MA-07 | 买家四个作用域均允许 | 将 `share/paid` 改为拒绝 | 只阻止后续付费 Share；免费 Share 和两类 Client Host 规则不受影响 |
| MA-08 | 任一当前白名单作用域 | 切换黑名单但不勾选风险确认,再勾选 | 未确认不能提交；确认后仅该作用域切换成功；重复保存黑名单不再要求风险确认 |
| MA-09 | 付费作用域为黑名单且无公共额度 | 未知买家分别租免费与付费商品 | 免费商品按各自免费作用域判断；付费商品因无额度被拒绝 |
| MA-10 | 黑名单 | 开启有限公共额度/尝试无限公共额度 | 有限额度需风险确认后可用；API 不提供无限公共额度且非法请求被拒绝 |
| MA-11 | 已有进行中服务 | 切换默认模式或修改产品规则 | 只影响新租用,不会隐式中断现有服务 |
| MA-12 | 外部系统持用户 API Token | 分别用 read/write scope 调准入接口,再提交旧 revision | 权限按 scope 隔离；过期 revision 返回冲突且不覆盖新设置 |
| MA-13 | 四个作用域配置不同模式与买家规则 | 逐一租用 Share / Host 的免费与付费报价 | 每次只读取匹配的 `(productKind, pricingKind)`，不存在跨作用域继承或串扰 |
| MA-14 | 已有多名可信买家 | 在买家表格搜索邮箱或用户 ID，再清空搜索 | 只过滤匹配行；未保存的其他行草稿不丢失，空结果显示搜索空态 |
| MA-15 | 同时修改多名买家的状态、准入或 CNY/USD 授信 | 观察全局按钮，先重置再重新修改并保存 | 无语义变更时「保存」和「重置」禁用；重置恢复服务端快照；保存一次提交全部脏行并刷新 revision；无限额度仍需逐项确认风险 |

### 9.3 统一市场账务(MB)

覆盖 `account-billing-page.tsx`。Share 与 Client Host 按「买方 + 供应商 + 币种」共用赊账账户；商品页不承担付款交互。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| MB-01 | 未登录 | 打开 `/account/billing` | 提示登录,不显示任何账户或收款快照 |
| MB-02 | 供应商 | 配置 CNY/USD 付款宽限时间 | 1-720 整数小时可保存；非法值被前端阻止 |
| MB-03 | 仅有收款资料或仅有付款宽限 | 发布对应币种付费 Share/Host | 均被阻止；两项都配置后才可发布 |
| MB-04 | 同供应商有多个付费 Share/Host | 查看应付账户 | 只出现一个同币种账户，列出多个服务、每日费率、健康时长试用与累计余额 |
| MB-05 | 尚未出账 | 查看账户及各商品页 | 不显示完整账号、地址或二维码；只显示付款方式种类和联系方式 |
| MB-06 | 有未出账余额 | 买方点「主动清账」并确认 | 生成一张聚合账单，关联服务暂停；账单金额和服务明细固定 |
| MB-07 | 有限额度累计用满 | 等待后台 reconcile | 自动生成一张多服务聚合账单并暂停相关服务，不生成单商品账单 |
| MB-08 | 最后一个服务结束且仍有余额 | 归还/回收/释放最后一个服务 | 生成最终聚合账单；停止时刻之后不再计费 |
| MB-09 | 买方有 open/overdue 账单 | 点「声明已付款」 | 弹窗显示冻结收款资料，可提交方式、参考号、凭证链接与备注；声明后仍待供应商确认 |
| MB-10 | 供应商看到付款声明 | 点拒绝并填写原因 | 账单恢复待付款；已过截止时间时全局赊账限制继续存在 |
| MB-11 | 供应商独立核实到账 | 点确认到账 | 账单结清、限制解除；仍有效且未永久关闭的服务恢复 |
| MB-12 | 账单超过截止时间 | 再租任意供应商的付费 Share/Host | 全局付费赊账被阻止；通过准入的免费商品仍可租用；仅声明或争议不解除限制 |
| MB-13 | 买方对账单有异议 | 发起争议,管理员分别测试维持/作废 | 每张账单只能有一个进行中争议；维持不解封，作废清除余额并恢复符合条件的服务 |
| MB-14 | 供应商决定停止关系 | 点「永久关闭赊账关系」 | 所有关联服务立即终止并生成/保留最终账单；结清或作废后不恢复，也不能再次建立付费租约 |
| MB-15 | 有多张历史账单 | 展开历史并加载更多 | 按 sequence 倒序分页；历史行保留各自服务明细、声明、争议和付款快照 |
| MB-16 | 买方/供应商/普通第三方/管理员 | 分别尝试付款、确认、拒绝、争议与裁决 | 买方只可声明/争议，供应商只可确认/拒绝/关闭，管理员只在裁决区有特权，第三方全部拒绝 |
| MB-17 | 账单快照含二维码资产，随后同 URL 换图 | 以资料所有者、账单买方、仅浏览商品的用户读取 | 旧账单继续返回旧图；仅资料所有者和该账单买方可读，挂牌、报价或活跃租约本身不授权资产 |
| MB-18 | 有限额度余额首次达到 80% | 运行 reconcile 并检查邮箱 | 买家和供应商各收到一次额度预警；同一周期不重复轰炸,结清后新周期可再次提醒 |
| MB-19 | 无限额度已有余额 | 多次运行 reconcile,再由供应商点「要求清账」 | 不自动出账；供应商请求后生成聚合账单、暂停服务并通知买家 |

---

## 10. 控制台与 Web 终端(T)

覆盖 `web-terminal/*`(6 文件)与 `client-console/*`(6 文件)。两者是**独立的窗口系统**,上限各自为 5。

### 10.1 Web 终端

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| T-01 | **主机属主** | 点行内终端图标 | 打开终端窗口并连接 |
| T-02 | 租客(非属主) | 观察该主机行 | **无终端入口** |
| T-03 | 管理员(非属主) | 观察 | **无终端入口** |
| T-04 | 已开终端 | 拖动标题栏 / 拖右下角 | 移动与缩放,限制在视口内;最小 420×280 |
| T-05 | 已开终端 | 点最小化 / 最大化 / 关闭 | 分别进 dock / 全屏 / 关闭 |
| T-06 | 已开终端 | 按 Esc | 最大化时还原;普通态关闭 |
| T-07 | 已开终端 | 点窗口外空白 | 最小化到 dock |
| T-08 | 开 5 个终端后再开 | 观察 | 提示达到上限(前端窗口上限 5) |
| T-09 | 有终端 | 切到非 shell 路由 | 自动最小化并 toast 提示 |
| T-10 | 有终端 | 刷新页面 | dock 中以「已挂起」态恢复,点击可重连 |
| T-11 | dock | 点垃圾桶 | 关闭全部终端 |
| T-13 | 终端空闲 20 分钟 | 观察 | 服务端断开(`IDLE_TIMEOUT`);连续 2 小时也会断(`MAX_SESSION_DURATION`) |
| T-14 | **同时开第 3 个终端** | 观察 | **前端允许开窗,但后端每用户并发会话上限为 2**,第 3 个连接应失败并给出可理解的错误 |
| T-15 | 打开终端后最小化,静置 >60 秒再点开 | 观察 | ticket TTL 为 60 秒,过期后需重新签发;不应静默卡住 |
| T-16 | 终端已连接 | 输入 `ls` 等命令 | 正常回显;中文输出不乱码 |
| T-17 | 终端已连接 | 调整窗口大小 | 终端列宽随之 resize,不错行 |
| T-18 | 终端已连接 | 断开网络再恢复 | 显示断开状态,不静默假死 |
| T-19 | 多个终端 | 点击不同窗口 | 焦点窗口置顶;dock 中焦点项高亮 |

> **T-14 是一处前后端上限不一致**:前端 `MAX_WEB_TERMINAL_WINDOWS = 5`(`web-terminal-manager.tsx:23`),后端 `MAX_SESSIONS_PER_OWNER = 2`(`src/client_market_terminal.rs:35`)。测试时确认第 3 个窗口的失败提示是否清晰;若表现为静默失败,应作为缺陷记录。

### 10.2 客户端控制台(iframe)

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| T-30 | client 有 tunnel URL | 点 Console | 打开 iframe 窗口,加载 client web 界面 |
| T-31 | client 无 tunnel | 观察 | 无 Console 入口 |
| T-32 | 已开控制台 | 拖动 / 缩放 / 最小化 / 最大化 / 关闭 | 与终端一致的窗口行为 |
| T-33 | 开 5 个控制台后再开 | 观察 | 达到上限(`MAX_CONSOLE_WINDOWS = 5`) |
| T-34 | 有控制台 | 刷新页面 | 从 sessionStorage 恢复窗口列表 |
| T-35 | 有控制台 | 跨标签页操作 | 窗口状态互不干扰(sessionStorage 是每标签页独立的) |
| T-36 | 同时开终端与控制台 | 观察 | 两套 dock 并存,互不覆盖;z-index 正确 |

---

## 11. Share 相关(S)

覆盖 `share-connect-dialog.tsx`、`share-edit-dialog.tsx`、`share-edit/*`、`share/share-page.tsx`、`drawer-panels.tsx`

### 11.1 连接弹窗

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| S-01 | 有 share | 打开 Connect | 显示 base URL + 复制 |
| S-02 | 未登录 | 打开 Connect | 琥珀提示 + 登录按钮 |
| S-03 | 无 `canViewSecret` | 打开 Connect | 红色提示 + 申请权限 mailto |
| S-04 | 有权限 | 复制 API key | 明文复制到剪贴板 |
| S-05 | 已绑定 app | 点「运行测试」 | 返回状态码/响应头/响应体;可展开收起;可复制 body 与 curl |
| S-06 | 文本类 app | 点「刷新用量」 | 触发用量刷新 |
| S-07 | 未绑定 | 观察 | 显示未绑定,按钮 disabled |

### 11.2 Share 编辑(Dashboard)

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| S-08 | `canManage` | 打开 Edit | 完整编辑表单 |
| S-09 | 非 `canManage` | 打开 View | 只读视图,仅关闭按钮 |
| S-10 | 编辑中 | 改任意字段 | 出现「重置」按钮;保存按钮由 disabled 变可用 |
| S-11 | 编辑中 | for_sale 改为 Free | 二次确认(danger) |
| S-12 | 编辑中 | 点某邮箱「设为属主」 | 二次确认(danger)后转移所有权 |
| S-13 | 编辑中 | 描述超 200 字 | 内联报错 + 字数计数;保存 disabled |
| S-14 | 编辑中 | Token / 并发限额填 0 或负数 | 内联报错 |
| S-15 | 编辑中 | 勾选「不限」 | 对应数字输入框 disabled |
| S-16 | 编辑中 | 定价填 0 或 101 | 报错(合法范围 1–100) |
| S-17 | 编辑中 | 观察售卖配置 | 只显示 Token Market 定价和访问范围,不出现旧 Share Market 类型或市场选择器 |
| S-18 | 编辑中 | 市场访问模式选「全部市场」 | 已选市场 chip 区隐藏;可点「切换为指定」恢复 |
| S-19 | 编辑中 | 添加/删除 shared-with 邮箱 tag | tag 增删;非法邮箱被拒 |
| S-20 | 支持 user grants | 编辑单用户额度 | 按用户设置 token/并发/过期 |
| S-21 | 编辑中 | 过期时间清空且未勾选「永久」 | 报错 |
| S-22 | 点重置 | 观察 | 所有字段回到打开时的值,重置按钮消失 |
| S-23 | 保存后客户端离线 | 观察 | 提示已入队,待客户端重连后生效 |
| S-24 | 保存后客户端在线 | 观察 | 同步生效;`OperationVerification` 在 30 秒内给出「已观察到生效」toast |
| S-25 | 保存被客户端拒绝 | 观察 share 卡片 | 显示拒绝态与错误 tooltip |

### 11.3 Share 抽屉与卡片

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| S-30 | 有 share | 打开 share 抽屉 | 首先展示运行诊断:状态、主因、持续时间、影响、证据 |
| S-31 | 承 S-30 | 观察用量条 | Token 用量与限额比例正确 |
| S-32 | 承 S-30 | 观察 app 支持卡 | Claude / Codex / Gemini 各自绑定状态 |
| S-33 | share 已挂牌 | 观察挂牌状态 chip | 与市场侧一致 |
| S-34 | share 有图片请求 | 打开图片请求记录 | 列表加载;鉴权图片正常显示 |
| S-35 | 有按邮箱用量 | 观察 | 每个邮箱的用量与限额状态 |

### 11.4 公开 Share 页(子域名)

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| S-40 | 匿名 | 访问 share 子域名页 | 只读状态卡 + 认证面板 |
| S-41 | 承 S-40 | 观察字段 | 名称、在线状态、属主邮箱、用量、子域名、app 类型、并发、过期 |
| S-42 | 承 S-40 | 填邮箱 + Router API token → 解锁 | `canManage` 时表单可编辑;凭据存 localStorage |
| S-43 | 填错误 token | 解锁 | 报错;凭据被清除 |
| S-44 | 已解锁 | 修改并保存设置 | 成功提示;字段与 Dashboard 编辑一致 |
| S-45 | 已解锁 | 点退出 | 清空凭据并刷新回只读态 |
| S-46 | share 离线 | 访问 | 状态 chip 显示离线 |

---

## 12. 聊天(CH)

覆盖 `chat/*`(5 文件)

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| CH-01 | client 支持聊天 | 打开聊天面板 | 历史消息加载;SSE 实时接收新消息 |
| CH-02 | 未登录 | 打开 | 可读历史;**输入框禁用或提示登录**,不能发送 |
| CH-03 | 已登录 | 发送消息 | 立即出现在列表 |
| CH-04 | 已登录 | 连续快速发送多条 | 触发限流时给出可理解提示(20 条/分钟) |
| CH-05 | 已登录 | 发送超长消息(>1000 字符) | 被拒或截断,有提示 |
| CH-06 | 匿名浏览过若干房间后登录 | 观察 | 本地访问记录一次性合并到服务端 |
| CH-07 | 管理员 | 删除某条消息 | 二次确认后删除,其他人视图同步消失 |
| CH-08 | 非管理员 | 观察他人消息 | 无删除入口 |
| CH-09 | 有未读 | 观察 client 卡片 | 显示未读角标;打开后清零 |
| CH-10 | 多个房间 | 切换房间 | 各房间未读独立;已读游标独立 |
| CH-11 | 打开中 | 断网再恢复 | SSE 断开有提示;恢复后补齐消息 |
| CH-12 | client 被清理后 | 打开原房间 | 只读归档态(保留 60 天) |
| CH-13 | 同一 Client 有多个 Share | 分别从 Share Market 行点群聊 | 均打开该 Client 的同一个公开房间,不创建 Share 房间 |
| CH-14 | Client Market Provider 与租客 | 分别从 Host 行/我的租用点群聊 | 双方进入同一 Client 房间;各自未读角标正确 |
| CH-15 | 完成租用、付款、争议、回收或清理 | 打开事件详情 | 显示完整双方邮箱、金额、收款资料、reference/note、凭证 URL、原因和安全原始错误 |
| CH-16 | 测试事件含 API Key/OAuth token/Cookie/Authorization/密码/secret/私钥或签名 URL | 查询 DB/API 并打开 UI | 凭据字段被拒绝,错误文本显示固定占位,危险 URL 不可见且不可点击 |
| CH-17 | 仅产生 Market/Billing 系统事件 | 等待超过聊天邮件聚合窗口 | Owner 不收到真人聊天提醒邮件;验证码、安全/Client 生命周期邮件不受影响 |
| CH-18 | outbox 中同时有失败事件与正常事件 | 运行 worker 并重试至上限 | 正常事件不被阻塞;失败事件最终 dead-letter;重复 source 只物化一次 |
| CH-19 | 系统消息包含同源收款图片 | 未登录打开图片;随后 Provider 更新收款资料 | 已发布图片可匿名读取且历史链接不失效;未发布图片仍返回未授权 |

---

## 13. 设置与管理(X)

覆盖 `settings/*`。**除标注外均为管理员专属。**

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| X-01 | 非管理员 | 打开 `/settings` | 提示无权限;**仍显示只读的版本面板与地图面板** |
| X-02 | 管理员 | 打开 | 左侧分组导航 + 表单;79 个字段 / 14 个分组 |
| X-03 | 管理员 | 修改任意字段 | 该分组出现未保存计数角标;顶部保存按钮显示总数 |
| X-04 | 管理员 | 点保存 | 返回更新/未变/需重启的键数量 |
| X-05 | 管理员 | 改需重启字段 | 字段显示「需重启」chip |
| X-06 | 管理员 | 点「保存并重启」 | 保存后重启;轮询 `/v1/healthz` 至恢复后自动刷新 |
| X-07 | 管理员 | 点「测试 Telegram」 | 成功/失败横幅 |
| X-08 | 管理员 | 打开持久化分组 | 显示 provision 公钥与 authorized_keys 行,可复制 |
| X-25 | 管理员 | 逐类字段验证控件类型 | bool→复选框;email_list/ip_list→多行文本域;secret→密码框且已设置时有提示;int/url/email→对应 input type |
| X-26 | 管理员 | 观察字段来源标注 | 每个字段显示取值来源(env / 文件 / 默认) |
| X-27 | 管理员 | 修改后不保存直接点「重新加载」 | 未保存变更被丢弃,回到服务端值 |
| X-28 | 管理员 | 填入非法值(如端口填字母) | 保存后服务端返回错误横幅,指出具体字段 |
| X-09 | 任意 | 版本面板 | 7 项信息;非管理员的二进制路径显示「仅管理员」 |
| X-10 | 管理员 | 点重启 | **1 步**确认后执行 |
| X-11 | 管理员 | 点升级 | **1 步**确认 → 弹窗 SSE 实时日志 → 完成后自动刷新 |
| X-12 | 管理员 | 点回滚 | **2 步**确认(第二步为「确定吗」)后执行 |
| X-13 | 无可回滚版本 | 观察回滚按钮 | disabled |
| X-29 | 升级进行中 | 观察日志弹窗 | 实时追加时间戳日志行;完成后显示 done |
| X-30 | 升级中断网 | 观察 | 显示断开提示并关闭 SSE,不无限转圈 |
| X-14 | 管理员 | 打开日志面板 | SSE 自动连接,状态 chip 显示 live |
| X-15 | 承 X-14 | 点暂停 / 恢复 | 暂停时丢弃新行(不缓冲);状态 chip 变 paused |
| X-16 | 承 X-14 | 点 4 个级别预设 + 5 个单选 | 过滤生效;显示 已过滤/总数;**不能取消最后一个级别** |
| X-17 | 承 X-14 | 点清空 / 重连 / 下载 | 分别清空缓冲 / 重建 SSE / 下载日志文件 |
| X-18 | 承 X-14 | 断开网络 | 状态变 disconnected + 错误 Alert |
| X-31 | 承 X-14 | 日志超过 1000 行 | 只保留最新 1000 行(`MAX_LINES`),不无限增长 |
| X-32 | 承 X-14 | 观察带 ANSI 颜色的日志 | 颜色正确渲染,不显示转义序列 |
| X-19 | 管理员 | 公告面板:改中英文 → 预览 | 弹窗渲染净化后的 HTML |
| X-20 | 承 X-19 | 保存 | 仅提交变更字段;标题显示「已修改」chip |
| X-33 | 承 X-19 | 填入含 `<script>` 的 HTML | 预览与前台展示均被净化,脚本不执行 |
| X-34 | 公告已启用 | 前台访问 | 与 A-16~A-19 联动验证 |
| X-21 | 管理员 | 地图面板:切换流量/热度、改起始像素 | 变更计入总未保存数,由顶部保存按钮提交 |
| X-22 | 非管理员 | 地图面板 | 所有控件 disabled,无重置按钮 |
| X-35 | 管理员 | 起始像素填非法值后失焦 | 回退到上一个合法值 |
| X-36 | 管理员 | 点「重置视口」 | 回到默认;已是默认时按钮 disabled |
| X-23 | 管理员 | 通知投递面板 | 两张表;死信行显示「重新入队」按钮 |
| X-24 | 承 X-23 | 点重新入队 | **无二次确认**,直接执行并刷新 |
| X-37 | 承 X-23 | 观察状态 chip | sent=成功色,dead_letter=危险色,retry/blocked_config=警告色 |
| X-38 | 承 X-23 | 观察收件人 | 邮箱已脱敏展示 |

---

## 14. 指标(N)

覆盖 `metrics/*`(5 文件)。**全页管理员专属。**

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| N-01 | 非管理员 | 打开 `/metrics` | 整页替换为无权限提示 |
| N-02 | 管理员 | 打开 | 初始骨架屏 → 数据渲染 |
| N-03 | 管理员 | 切 5 个时间范围 | 15m/1h/6h/24h/7d;步长相应为 15s/30s/1m/5m/15m |
| N-04 | 管理员 | 开自动刷新 | 每 5 秒静默刷新;关闭后停止 |
| N-05 | 管理员 | 切 5 个 tab | 概览 / 主机 / 路由 / LLM / 事件 各自加载 |
| N-06 | 数据陈旧或关闭 | 观察图表标题 | 显示琥珀色 stale/disabled 角标 |
| N-07 | 管理员 | 点「清空指标」 | 二次确认(danger)→ 成功 Alert 5 秒后自动消失 |
| N-08 | 概览 tab | 观察 | 4 个实时 KPI + 8 个指标卡(含迷你图)+ 系统风险趋势图 + 最近 6 条事件 |
| N-09 | 主机 tab | 观察 | 4 个 KPI + 主机性能趋势 + 进程面板 + 磁盘列表 + 主机信息 |
| N-10 | 路由 tab | 观察 | 4 个 KPI + 路由/监听趋势 + 代理错误计数增量 + 计数表 |
| N-11 | LLM tab | 观察 | 5 个 KPI + 请求/错误趋势 + Token 趋势 + 模型替换面板 + Top 消费者表 |
| N-12 | 事件 tab | 观察 | 完整事件列表 |
| N-13 | 指标库为空 | 打开 | 空态而非报错;图表显示无数据 |
| N-14 | 承 N-07 | 清空后观察 | 各 tab 数据归零,不残留旧值 |

---

## 15. Clients 抽屉与诊断(D)

覆盖 `drawer-panels.tsx`、`operation-verification.tsx`、`provision-job-log.tsx`、`client-upgrade-button.tsx`

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| D-01 | 有 client | 打开 client 抽屉 | 首屏为运行诊断:状态 → 主因 → 持续时间 → 影响 → 证据 |
| D-02 | 承 D-01 | 点证据条目 | 可定位到健康时间线或相关详情 |
| D-03 | 承 D-01 | 观察 share 列表 | 每行有 Edit 入口 |
| D-04 | client 离线 | 打开抽屉 | 离线原因与持续时间可读 |
| D-05 | 有 market | 打开 market 抽屉 | 诊断 + 挂牌 share + 优先级面板 |
| D-06 | 保存 share 配置后 | 观察 30 秒 | `OperationVerification` 区分「API 提交成功」与「Dashboard 已观察到生效」两个 toast |
| D-07 | 配置被拒绝 | 观察 | 明确的拒绝 toast,而非静默 |
| D-08 | 有可升级 client | 点升级 | 二次确认 → 进度可见 |
| D-09 | 承 D-08 | 升级中刷新页面 | 状态从 `cc-switch-router-client-upgrade-state` 恢复 |
| D-10 | 任意开通/清理作业 | 观察 `ProvisionJobLog` | 日志实时追加,可滚动;失败时高亮 |

---

## 16. 跨界面(G)

| ID | 范围 | 检查项 |
|---|---|---|
| G-01 | 全站 | 中英文各跑一遍主流程,无未翻译串、无中英混排 |
| G-02 | 全站 | 1440 / 1024 / 768 / 375 四档宽度;**主机表 `min-w-[56rem]`,375px 下需横向滚动可用** |
| G-03 | 全站 | 键盘 Tab 可达所有主操作;Markets 行支持 Enter/Space |
| G-04 | 主机表 | 屏幕阅读器读出表格 caption;4 个批量按钮可区分 |
| G-05 | 全站 | 所有破坏性操作均有二次确认(例外见下) |
| G-06 | 全站 | 断网后各页面显示错误态而非白屏;恢复后可重试 |
| G-07 | 全站 | 慢速网络下 loading 态可见,不出现闪烁空态 |
| G-08 | 全站 | 弹窗打开时焦点进入弹窗;关闭后焦点回到触发元素 |
| G-09 | 全站 | Esc 可关闭所有非阻塞弹窗;阻塞态(如创建中)不响应 Esc |
| G-10 | 全站 | 切换语言后,已打开的弹窗与表格即时更新文案 |
| G-11 | 全站 | 同一账号多标签页操作,登录/登出状态同步 |
| G-12 | 全站 | 清空 localStorage 后首次进入,各页面均有合理默认态 |

**已知无二次确认的操作**(设计如此,验证时不算缺陷):日志面板「清空」(纯前端缓冲)、通知投递「重新入队」。

---

## 17. 新建 Client 报价流(Q)

`create-client-dialog.tsx` 逻辑最密集,单列一组。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| Q-01 | 未登录 | 点「选择主机」 | 触发登录,不发报价请求 |
| Q-02 | 已登录 | 在线模式选供给方 + 区域 + 数量 | 数量上限 2;容量不足时提示且按钮 disabled |
| Q-03 | 已登录 | 手动模式 | 只显示安装命令 + 复制按钮 |
| Q-04 | 从主机行进入 | 观察 | 锁定该主机,不显示模式 tab |
| Q-05 | 获取报价后 | 观察倒计时 | 120 秒起算;>60s 常态、≤60s 琥珀、≤30s 红色 |
| Q-06 | 报价中 | 填子域名 | 350ms 防抖校验可用性;边框红/绿 + 状态文案 |
| Q-07 | 报价中 | 点骰子 | 生成合法随机子域名 |
| Q-08 | 报价中 | 填非法子域名(<6 位 / 含 `--` / 保留字 admin) | 校验拒绝 |
| Q-09 | 报价中 | 两项填相同子域名 | 重复检测报错 |
| Q-10 | 报价中,已填写内容 | **等待过期** | 回到表单;提示「已保留填写内容」;**再次报价时子域名与密码按主机自动回填** |
| Q-11 | 报价中 | 点返回 | 取消报价,主机释放回池 |
| Q-12 | 报价有效期内 | 点确认创建 | 重新校验全部子域名后提交 |
| Q-13 | 创建中 | 尝试关闭 | 不可关闭;显示各主机的 provisioning 日志 |
| Q-14 | 部分失败 | 观察完成态 | 琥珀色汇总 + 部分回滚提示 |

---

## 18. 反查索引:改了文件 → 重跑哪些用例

> **编号约定**:各小节之间有意留有编号空档(如 H-07 之后跳到 H-10),便于插入新用例时不必重排。下表的 `A~B` 表示该区间内**已存在**的用例,空档不代表遗漏。

| 文件 | 用例 |
|---|---|
| `layout/app-shell.tsx` | A-01, A-07~A-15, A-20 |
| `auth/login-dialog.tsx`, `auth-provider.tsx` | A-02~A-06, A-12, A-21~A-26 |
| `announcement/*` | A-16~A-19 |
| `dashboard/live-map.tsx` | C-02~C-06 |
| `dashboard/client-board.tsx` | C-07~C-16, C-22 |
| `dashboard/share-card.tsx` | C-17~C-20 |
| `dashboard/drawer-panels.tsx` | C-13, C-18, M-05, M-09 |
| `dashboard/markets-table.tsx` | M-01~M-15 |
| `dashboard/share-market-page.tsx` | SM-01~SM-16, SM-E2E-01~SM-E2E-07 |
| `dashboard/account-share-page.tsx` | AS-01~AS-08 |
| `dashboard/account-client-page.tsx` | 账户 Client 只读监控(镜像 AS) |
| `dashboard/client-market-page.tsx` | H-01~H-18(归属/筛选/排序/分页), H-60~H-71(选择与批量), H-80~H-84(导入导出) |
| `client-market/host-utils.ts` | H-10~H-13, H-42(矩阵), H-64 |
| `client-market/host-row.tsx` | H-40~H-51, T-01~T-03 |
| `client-market/add-host-dialog.tsx` | H-20~H-29 |
| `client-market/host-offer-dialog.tsx` | H-43, H-26 |
| `client-market/host-sort-header.tsx` | H-12~H-15 |
| `client-market/use-batch-operations.ts` | H-60~H-71 |
| `client-market/rentals-page.tsx` | R-01~R-04, R-12, R-34 |
| `client-market/my-rentals-panel.tsx` | R-03, R-04, R-20~R-22, R-32 |
| `dashboard/client-market-rental-banner.tsx` | R-05~R-07, R-11, R-30, R-35 |
| `client-market/release-rental-action.tsx` | R-23~R-35 |
| `dashboard/create-client-dialog.tsx` | C-19, H-40, 见 §17 |
| `dashboard/web-terminal/*` | T-01~T-19 |
| `dashboard/client-console/*` | C-15, T-30~T-36 |
| `dashboard/share-edit-dialog.tsx`, `share-edit/*` | S-08~S-25 |
| `dashboard/share-connect-dialog.tsx` | S-01~S-07 |
| `dashboard/account-page.tsx` | AC-01~AC-18 |
| `dashboard/account-billing-page.tsx` | MB-01~MB-17 |
| `dashboard/operation-verification.tsx` | S-24, D-06, D-07 |
| `dashboard/provision-job-log.tsx` | H-45, D-10, Q-13 |
| `dashboard/client-upgrade-button.tsx` | C-21, D-08, D-09 |
| `share/share-page.tsx` | S-40~S-46 |
| `settings/settings-page.tsx` | X-01~X-08, X-25~X-28 |
| `settings/version-panel.tsx` | X-09~X-13, X-29, X-30 |
| `settings/logs-panel.tsx` | X-14~X-18, X-31, X-32 |
| `settings/announcement-panel.tsx` | X-19, X-20, X-33, X-34 |
| `settings/map-display-panel.tsx` | X-21, X-22, X-35, X-36 |
| `settings/client-notification-deliveries-panel.tsx` | X-23, X-24, X-37, X-38 |
| `metrics/*` | N-01~N-14 |
| `chat/*` | CH-01~CH-19 |
| `common/confirm-alert-dialog.tsx` | G-05, G-08, G-09 |
| `common/compact-region-multi-select.tsx` | C-09, H-07, H-14, H-15 |
| `common/copyable-code-field.tsx` | H-24, X-08 |
| `common/authenticated-image.tsx` | S-34, MB-09, MB-17 |
| `common/payment-method-icons.tsx` | AC-03~AC-09, R-06, MB-05, MB-09 |
| `common/country-flag.tsx` | C-03, H-14, R-03 |
| `lib/client-market-refresh.ts` | H-23(数据刷新不丢状态), C-22, R-12 |
| `lib/i18n.ts` | G-01, G-10 + 改动键所属界面 |
| `lib/dashboard-nav.ts` | A-20 |
| `lib/use-persistent-state.ts` | §3 全部持久化用例, G-12 |
| `lib/api.ts` | 见 §19 覆盖核对表 |

---

## 19. 覆盖核对:API → 用例

`lib/api.ts` 按域导出端点函数(`parseJson` 为跨模块辅助函数)。改端点名、路径或调用方时更新此表；不维护易失真的总数。

| 域 | 代表 API 函数 | 覆盖用例 |
|---|---|---|
| Admin(设置/版本/日志/公告/地图/通知/市场管理) | `getSettings*`、`saveSettings`、`updateMarket*` | X-02~X-08, X-10~X-13, X-17, X-19~X-24, M-10~M-14 |
| Client Market(主机/作业/报价/终端/子域名) | `getClientMarketHosts`、`createClientMarketQuote`、`commitClientMarketQuote` | H-20~H-29, H-43~H-48, H-80~H-84, T-01, Q-02, Q-06, Q-11, Q-12 |
| Client 租用生命周期 | `getMyClientMarketRentals`、`releaseClientMarketRental`、`cleanupClientMarketProviderRental` | R-01~R-35, H-44~H-48 |
| 市场准入与授信 | `getMarketAccessDashboard`、`updateMarketAccessPolicy`、`upsertMarketCounterparty`、`updateMarketCounterparty`、`updateMarketCounterpartyCredit`、`updateMarketPublicCredit` | MA-01~MA-15 |
| 统一市场账务 | `getMarketBillingDashboard`、`settleMarketBillingAccount`、`requestMarketBillingSettlement`、`declareMarketBillingPayment`、`confirmMarketBillingPayment`、争议/作废端点 | MB-01~MB-19 |
| 聊天 | `getClientChat*`、`postClientChatMessage` | CH-01~CH-19 |
| 指标 | `getMetrics*`、`getLlmMetrics*` | N-02~N-14 |
| Shares | `updateShareSettings`、`getShareUsageByEmail`、`refreshShareUsage` | S-05, S-06, S-08~S-25, S-34, S-35 |
| 账户收款资料 | `getAccountPaymentProfile`、`updateAccountPaymentProfile` | AC-03~AC-16, MB-03, MB-17 |
| Dashboard | `getDashboard`、`getMapDisplay` | C-01, C-22 |
| Installations 升级 | `upgradeClientInstallation`、`getClientInstallationUpgradeStatus` | C-21, D-08, D-09 |
| 用户 API Token | `getUserApiToken`、`resetUserApiToken` | A-10, A-11 |
| Markets 优先级 | `getMarketSharePriority` | M-09 |
| Share Market | `getShareMarket*`、`*ShareMarket*` | SM-01~SM-16, SM-E2E-01~SM-E2E-07 |
| 其他(regions / 公告读取) | `getRegions`、`getAnnouncement` | A-14, A-16 |

认证相关在 `lib/auth.ts`(非 `api.ts`):`requestEmailCode` / `verifyEmailCode` / `refreshAccessToken` / `sessionStatus` / `logoutSession` / `ensureInstallationIdentity` → 用例 A-02~A-06, A-12。

### 覆盖缺口

**当前无已知 UI 不可达的市场准入或账务能力。** 准入与授信端点由 `/account/market-access` 调用,统一账务端点由 `/account/billing` 调用；两者也支持 scoped 用户 API Token。静态审计会阻止旧单商品支付、旧独立封禁和供应商全局额度契约重新进入代码库。

> 若后续再出现"接口写好了但没有 UI 入口"的情况,应在此处登记,并说明由后端测试还是手工调接口验证 —— 手动 UI 清单对它们无能为力。

---

## 20. 一轮完整回归的建议顺序

共 **386 条用例**。单人跑完约需 5–6 小时。按角色分轮次,减少环境切换:

| 轮次 | 环境 | 用例 | 约计 |
|---|---|---|---|
| 1. 匿名 | `DEV_AUTH_BYPASS=0`,不登录 | A-01, C-01~C-06, M-01~M-08, H-06, H-32, S-40, S-41, S-46, CH-02, N-01, X-01 | 25 分钟 |
| 2. 普通用户 | 登录,名下无主机无租用 | A-02~A-20, AC-01~AC-18, H-01, H-02, H-51, SM-16, R-01, R-02, C-07~C-23 | 60 分钟 |
| 3. 供给方 | 名下有多状态主机 | H-03~H-05, H-07, H-10~H-18, H-20~H-29, H-40~H-50, H-60~H-84, T-01, T-04~T-19, T-30~T-36, Q-01~Q-14, D-10 | 120 分钟 |
| 4. 租客 | 有租用中 Client | R-03~R-12, R-20~R-35, T-02, H-30, S-42~S-45, D-01~D-04 | 55 分钟 |
| 5. 管理员 | 邮箱在 `ADMIN_EMAILS` | X-02~X-38, N-02~N-14, M-09~M-15, CH-07, H-31, T-03, D-05~D-09, S-08~S-35 | 90 分钟 |
| 6. 跨界面 | 任意角色 | G-01~G-12 | 30 分钟 |

**冒烟子集**(每次提交前跑,约 15 分钟):

`A-04`(登录)· `C-01`(总览渲染)· `H-01`(默认只看自己 + 故障优先排序)· `H-11`(严重度序)· `H-51`(白名单引导)· `SM-16`(聊天室引导)· `H-60`(选择模式)· `R-03`(租用列表)· `R-23`(释放与数据丢失确认)· `MB-04`(聚合账户)· `MB-09`(付款声明)· `Q-05`(报价倒计时)· `Q-10`(过期保留草稿)· `X-02`(设置表单)· `G-01`(中英文)

**建议按轮次记录结果**,而不是逐条打勾——失败项记 用例 ID + 实际现象 + 截图,便于回归定位。

---

## 21. 变更记录

本文件应随功能变更更新。重大调整在此登记,便于判断清单是否落后于代码。

| 日期 | 变更 |
|---|---|
| 2026-07-26 | 首版。309 条用例,覆盖 8 条路由 / 84 个组件 / 76 个 API 端点。同步删除 12 个无调用方的 API 函数与 66 个 `board.*` i18n 键。 |
| 2026-07-27 | 租用释放改造:释放入口从付款弹窗内移到租用卡片常驻,新增账单不回滚说明与清理进度弹窗。R 组 12 → 25 条,总数 322。 |
| 2026-07-30 | Share/Client Market 改为供应商级统一后付费；删除单商品预付、续费和退款交互，新增 MB-01~MB-17、支付快照权限及旧契约静态审计。 |
| 2026-07-30 | Share/Client Host 免费与付费租用统一改为默认白名单；新增邮箱预授权、产品规则、买家级有限/无限授信、风险确认的黑名单模式与有限公共额度,扩展 MA-01~MA-12、MB-18~MB-19。 |
| 2026-07-30 | 未获供应商准入时保留 Share「租用」/Client Host「新建」入口并改为联系 Owner 的授权引导弹窗；Share 跳转 Client 聊天室,Host 展示邮件与公开联系方式,新增 SM-16、H-51。 |
| 2026-07-31 | 免费与付费准入拆为四个独立作用域，免费默认黑名单、付费默认白名单；免费 Share/Host 新增 1–365 天或永久期限、报价快照、临期事件和到期安全回收，扩展 SM/H/AS/MA 回归用例。 |
| 2026-07-31 | 可信买家管理改为可搜索表格和统一草稿保存；新增全局重置/保存门控以及多买家批量编辑回归用例 MA-14~MA-15。 |
