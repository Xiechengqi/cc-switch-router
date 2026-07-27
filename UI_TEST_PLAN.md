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

现状:仓库**没有前端自动化测试**(无 jest/vitest/playwright),后端有 473 个 Rust 内联测试。所以前端行为的唯一防线就是这份手动清单。

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
| `/client-market` 主机表 | ✅ 只读 | ✅ 空表 | ✅ 全功能 | ✅ 空表 | ✅ 无特权 |
| `/rentals` | ❌ 提示登录 | ✅ 空 | ✅ 空 | ✅ 有数据 | ✅ |
| `/account` | ❌ | ✅ | ✅ | ✅ | ✅ |
| `/settings` | ❌ | 部分只读 | 部分只读 | 部分只读 | ✅ 全功能 |
| `/metrics` | ❌ | ❌ 提示 | ❌ 提示 | ❌ 提示 | ✅ |
| Share 页(子域名) | ✅ 只读 | ✅ | ✅ | ✅ | ✅ |

> **管理员在 Client Market 无特权**。主机操作只认 host owner,管理员不被提升(`src/client_market.rs:889` 注释明确)。用例 H-31 覆盖。

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
| C-17 | 有收款资料 | 点地址复制 | 复制到剪贴板 |
| C-18 | 有 share | 点 share 卡片体 | 打开 share 详情抽屉 |
| C-19 | share 非 disabled | 点 Connect | 打开连接弹窗(见 §11) |
| C-20 | 有 share | 点 Edit / View | `canManage` 时可编辑,否则只读视图 |
| C-21 | 有待应用编辑 | 观察 share 卡片 | 显示「待应用」;被拒绝时显示错误 tooltip |
| C-22 | 有可升级 client | 点升级按钮 | 弹二次确认后开始升级 |
| C-23 | 任意 | 保持页面 30 秒 | 数据自动刷新,展开态/筛选态不被重置 |
| C-24 | 配置了 Telegram | 观察页脚 | 显示 Telegram 链接,新标签打开 |

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
| M-10 | `canManage` 且非 share market | 点 Edit | 打开编辑弹窗 |
| M-11 | 承 M-10 | 勾选维护模式 + 填消息 → 保存 | 保存成功;消息框在未开启维护时 disabled |
| M-12 | 承 M-10 | 勾选若干 share → 停用所选 | 对应 share 进入停用集合 |
| M-13 | 承 M-10 | 点 全部启用 / 全部停用 | 批量生效 |
| M-14 | 有阻塞状态 | 点单条 Release / 全部 Release | 逐条释放 |
| M-15 | 非 `canManage` | 打开抽屉 | 无 Edit 按钮 |

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
| H-29 | 填价格但不填周期 | 提交 | 校验拒绝(价格与周期必须同时填或同时空) |

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
| H-47 | allocated 且非自己租用 | 未付款清理 | 红色项;二次确认警告会封禁该租客 |
| H-48 | 无 installation 且 idle/disabled/abnormal | Delete | 二次确认(danger)后删除 |
| H-49 | 清理进行中 | 尝试关闭进度弹窗 | 关闭按钮 disabled;任务成功或失败后才可关 |
| H-50 | 有备注的主机 | 观察 | 备注以子行展示;**无备注则不出现空子行** |

**操作可用性矩阵**(行=主机状态,列=菜单项;均需 host owner):

| 状态 | Edit Offer | Reverify | Cleanup | 未付款清理 | Delete |
|---|:--:|:--:|:--:|:--:|:--:|
| idle | ✅ | — | — | — | ✅ |
| allocated | ✅ | — | ✅ | ✅ | — |
| draining | ✅ | — | ✅(重试) | — | — |
| unreachable | ✅ | ✅ | ✅(重试) | — | — |
| abnormal | ✅ | ✅ | — | — | ✅ |
| disabled | ✅ | ✅ | — | — | ✅ |

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

覆盖 `client-market/rentals-page.tsx`、`my-rentals-panel.tsx`、`client-market-billing-banner.tsx`

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| R-01 | 未登录 | 打开 `/rentals` | 提示登录,不发数据请求 |
| R-02 | 登录但无租用 | 打开 | 虚线空态卡片 |
| R-03 | 有租用 | 打开 | 每条租用一张卡:国旗、子域名、主机状态、供给方邮箱 + 账单区 |
| R-04 | **既出租又租用** | 打开 | **只显示自己租的**,自己出租给别人的不出现 |
| R-05 | 账单 payment_due | 观察 | 显示紧急度 chip 与「支付」按钮 |
| R-06 | 承 R-05 | 点支付 | 弹窗显示供给方收款方式、二维码、金额、截止时间与倒计时 |
| R-07 | 承 R-06 | 点「我已支付」 | 二次确认(warning)→ 成功 toast → 弹窗关闭 |
| R-08 | 承 R-06 | 点「释放 Client」 | 二次确认(danger)→ 开始释放 toast |
| R-09 | 状态 releasing | 观察 | 显示释放中,无可点操作 |
| R-10 | 状态 release_failed | 观察 | 错误文案 + 「重试释放」按钮 |
| R-11 | 供给方中途改价 | 尝试支付 | 被拒绝并提示重新确认价格(金额回显校验) |
| R-12 | 任意 | 保持 20 秒 | 自动刷新;`/client-market` 页**不再请求账单接口** |

---

## 9. 账户(AC)

覆盖 `account-page.tsx`。收款资料直接影响付费报价能否发布,是 Client Market 的前置。

| ID | 前置 | 步骤 | 预期 |
|---|---|---|---|
| AC-01 | 已登录 | 打开 `/account` | 显示收款资料区与封禁列表区 |
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
| AC-15 | 有封禁记录 | 观察列表 | 显示被封禁的租客与时间 |
| AC-16 | 承 AC-15 | 点解除 | 该租客解除;解除后其可再次租用本人主机 |
| AC-17 | 无封禁 | 观察 | 空态文案 |
| AC-18 | 修改收款资料后 | 让租客侧刷新账单 | 租客支付时因 `paymentProfileUpdatedAt` 变化被要求重新确认(与 R-11 呼应) |

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
| S-17 | 编辑中 | 售卖类型选 Share Market 但未选市场 | 报错,保存 disabled |
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
| `auth/login-dialog.tsx`, `auth-provider.tsx` | A-02~A-06, A-12 |
| `announcement/*` | A-16~A-19 |
| `dashboard/live-map.tsx` | C-02~C-06 |
| `dashboard/client-board.tsx` | C-07~C-17, C-23 |
| `dashboard/share-card.tsx` | C-18~C-21 |
| `dashboard/drawer-panels.tsx` | C-13, C-18, M-05, M-09 |
| `dashboard/markets-table.tsx` | M-01~M-15 |
| `dashboard/client-market-page.tsx` | H-01~H-18(归属/筛选/排序/分页), H-60~H-71(选择与批量), H-80~H-84(导入导出) |
| `client-market/host-utils.ts` | H-10~H-13, H-42(矩阵), H-64 |
| `client-market/host-row.tsx` | H-40~H-50, T-01~T-03 |
| `client-market/add-host-dialog.tsx` | H-20~H-29 |
| `client-market/host-offer-dialog.tsx` | H-43, H-26 |
| `client-market/host-sort-header.tsx` | H-12~H-15 |
| `client-market/use-batch-operations.ts` | H-60~H-71 |
| `client-market/rentals-page.tsx` | R-01~R-04, R-12 |
| `client-market/my-rentals-panel.tsx` | R-03, R-04 |
| `client-market-billing-banner.tsx` | R-05~R-11 |
| `dashboard/create-client-dialog.tsx` | C-19, H-40, 见 §17 |
| `dashboard/web-terminal/*` | T-01~T-19 |
| `dashboard/client-console/*` | C-15, T-30~T-36 |
| `dashboard/share-edit-dialog.tsx`, `share-edit/*` | S-08~S-25 |
| `dashboard/share-connect-dialog.tsx` | S-01~S-07 |
| `dashboard/account-page.tsx` | AC-01~AC-18 |
| `dashboard/operation-verification.tsx` | S-24, D-06, D-07 |
| `dashboard/provision-job-log.tsx` | H-45, D-10, Q-13 |
| `dashboard/client-upgrade-button.tsx` | C-22, D-08, D-09 |
| `share/share-page.tsx` | S-40~S-46 |
| `settings/settings-page.tsx` | X-01~X-08, X-25~X-28 |
| `settings/version-panel.tsx` | X-09~X-13, X-29, X-30 |
| `settings/logs-panel.tsx` | X-14~X-18, X-31, X-32 |
| `settings/announcement-panel.tsx` | X-19, X-20, X-33, X-34 |
| `settings/map-display-panel.tsx` | X-21, X-22, X-35, X-36 |
| `settings/client-notification-deliveries-panel.tsx` | X-23, X-24, X-37, X-38 |
| `metrics/*` | N-01~N-14 |
| `chat/*` | CH-01~CH-12 |
| `common/confirm-alert-dialog.tsx` | G-05, G-08, G-09 |
| `common/compact-region-multi-select.tsx` | C-09, H-07, H-14, H-15 |
| `common/copyable-code-field.tsx` | H-24, X-08 |
| `common/authenticated-image.tsx` | S-34, R-06 |
| `common/payment-method-icons.tsx` | AC-03~AC-09, R-06 |
| `common/country-flag.tsx` | C-03, H-14, R-03 |
| `lib/client-market-refresh.ts` | H-23(数据刷新不丢状态), C-23, R-12 |
| `lib/billing-urgency.ts` | R-05, R-11 |
| `lib/i18n.ts` | G-01, G-10 + 改动键所属界面 |
| `lib/dashboard-nav.ts` | A-20 |
| `lib/use-persistent-state.ts` | §3 全部持久化用例, G-12 |
| `lib/api.ts` | 见 §19 覆盖核对表 |

---

## 19. 覆盖核对:API → 用例

`lib/api.ts` 当前导出 **76 个端点函数**(另有 `parseJson` 为跨模块辅助函数),**全部有组件调用**。改 `lib/api.ts` 时更新此表。

| 域 | 端点数 | 覆盖用例 |
|---|---:|---|
| Admin(设置/版本/日志/公告/地图/通知/市场管理) | 20 | X-02~X-08, X-10~X-13, X-17, X-19, X-20, X-21, X-23, X-24, M-10~M-14 |
| Client Market(主机/作业/报价/终端/子域名) | 20 | H-20~H-29, H-43~H-48, H-80~H-84, T-01, Q-02, Q-06, Q-11, Q-12 |
| 聊天 | 10 | CH-01~CH-12 |
| 指标 | 7 | N-02~N-14 |
| Shares | 6 | S-05, S-06, S-08~S-25, S-34, S-35 |
| 账户(收款资料/封禁) | 4 | AC-03~AC-16 |
| Dashboard | 2 | C-01, C-23 |
| Installations 升级 | 2 | C-22, D-08, D-09 |
| 用户 API Token | 2 | A-10, A-11 |
| Markets 优先级 | 1 | M-09 |
| 其他(regions / 公告读取) | 2 | A-14, A-16 |
| 账单(`getMyClientMarketBilling` / `declareClientMarketPayment`) | 2 | R-05~R-11 |

认证相关在 `lib/auth.ts`(非 `api.ts`):`requestEmailCode` / `verifyEmailCode` / `refreshAccessToken` / `sessionStatus` / `logoutSession` / `ensureInstallationIdentity` → 用例 A-02~A-06, A-12。

### 覆盖缺口

**当前无 UI 不可达的后端能力。** 此前存在的 12 个无调用方函数(留言板整套 7 个、`getMetricsHostStatus`、`getLlmMetricsSnapshot`、`getClientMarketSupplySummary`、单实例 `getClientMarketBilling`、无 reason 版 `cleanupClientMarketClient`)已随对应的 66 个 `board.*` i18n 键与 3 个 board 类型一并删除。

> 若后续再出现"接口写好了但没有 UI 入口"的情况,应在此处登记,并说明由后端测试还是手工调接口验证 —— 手动 UI 清单对它们无能为力。

---

## 20. 一轮完整回归的建议顺序

共 **309 条用例**。单人跑完约需 4–5 小时。按角色分轮次,减少环境切换:

| 轮次 | 环境 | 用例 | 约计 |
|---|---|---|---|
| 1. 匿名 | `DEV_AUTH_BYPASS=0`,不登录 | A-01, C-01~C-06, M-01~M-08, H-06, H-32, S-40, S-41, S-46, CH-02, N-01, X-01 | 25 分钟 |
| 2. 普通用户 | 登录,名下无主机无租用 | A-02~A-20, AC-01~AC-18, H-01, H-02, R-01, R-02, C-07~C-24 | 60 分钟 |
| 3. 供给方 | 名下有多状态主机 | H-03~H-05, H-07, H-10~H-18, H-20~H-29, H-40~H-50, H-60~H-84, T-01, T-04~T-19, T-30~T-36, Q-01~Q-14, D-10 | 120 分钟 |
| 4. 租客 | 有租用中 Client | R-03~R-12, T-02, H-30, S-42~S-45, D-01~D-04 | 40 分钟 |
| 5. 管理员 | 邮箱在 `ADMIN_EMAILS` | X-02~X-38, N-02~N-14, M-09~M-15, CH-07, H-31, T-03, D-05~D-09, S-08~S-35 | 90 分钟 |
| 6. 跨界面 | 任意角色 | G-01~G-12 | 30 分钟 |

**冒烟子集**(每次提交前跑,约 15 分钟):

`A-04`(登录)· `C-01`(总览渲染)· `H-01`(默认只看自己 + 故障优先排序)· `H-11`(严重度序)· `H-40`(新建入口)· `H-60`(选择模式)· `R-03`(租用列表)· `Q-05`(报价倒计时)· `Q-10`(过期保留草稿)· `X-02`(设置表单)· `G-01`(中英文)

**建议按轮次记录结果**,而不是逐条打勾——失败项记 用例 ID + 实际现象 + 截图,便于回归定位。

---

## 21. 变更记录

本文件应随功能变更更新。重大调整在此登记,便于判断清单是否落后于代码。

| 日期 | 变更 |
|---|---|
| 2026-07-26 | 首版。309 条用例,覆盖 8 条路由 / 84 个组件 / 76 个 API 端点。同步删除 12 个无调用方的 API 函数与 66 个 `board.*` i18n 键。 |
