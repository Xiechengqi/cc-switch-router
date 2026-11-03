# Clients 首页与 Share 车位入口

## 路由和首页

Router 主域名的 `/` 直接渲染 Clients，不再先跳转 `/clients/`，也不再根据账号首页偏好跳转市场。Clients 和市场共用持久 Dashboard 布局。

- `/clients`、`/clients/` 是精确旧地址别名：Router HTTP GET/HEAD 返回 308 到 `/` 并保留查询；静态前端别名同时保留查询和锚点。不会匹配 `/client-market/`、`/clients-extra` 或子路径。
- 已有任务链接继续有效：根路径的充值、C2C 和旧市场抽屉参数仍转到对应页面；聊天、Share/Client 定位和新的车位参数留在首页。
- `/v1/ui/context` 明确区分 Router 和在线 Share 域名。未知域名不猜测为 Router；请求失败有重试入口。Client Web 的根路径及旧地址仍由代理处理，不执行 Router 别名跳转。
- 移除首页选择 UI、运行时代码、`/v1/auth/home` 和旧引导测试。历史迁移 94 与数据库偏好字段保持原样，不改历史校验和、不清空用户数据。
- 无资源时展示内联选购入口，不阻断用户进入客户端页。

## Share 卡片入口

Clients 的 Share 卡片右下角使用轻量批量查询决定入口；按钮阻止冒泡，不打开 Share 侧栏。

| 本人状态 | 入口 |
| --- | --- |
| 没有租约，公开挂售且有空位 | 租用 |
| 有占位中的租约，包括授权/回收中 | 我的车位 |
| 已释放车位，但仍有未结退租或服务争议 | 查看结算 |
| 满员且无本人租约、下架且无本人关系、Owner 或普通直接授权用户 | 不展示新的租用入口 |

弹窗按“我的车位 → 空车位 → 已租用／暂不可用”排列，历史租约与结算独立列出。当前车位始终包含在详情中，不会被超过一页的旧结算记录挤掉。可读取所有公开车位，但不返回其他租客的 subscription 或个人财务信息。

本人车位复用既有租约详情：冻结条款、用量、自动续租、到期退出、立即退租、结算和服务争议。租用复用 Share Market 报价与确认组件。后端与唯一索引仍是“同账号同 Share 最多一个有效租约”的最终约束；前端不会因未加载全部历史而放开第二次租用。

满员、下架、Share 删除后，本人仍能查看有效租约／未结事项。在“我的”中，缺少正常 Share 卡片的租约显示轻量恢复卡片，计入已加载的“我的”数量；只显示真实子域名或 Share ID，不伪造资源或连接入口。恢复列表分页独立于历史列表最近五条记录；更多记录通过“加载更多”取得。已完成事项不长期混入恢复卡片，完整历史仍在市场与账号租用历史中。

## 新增只读 API

- `GET /v1/share-market/client-entries?shareIds=…`：最多 100 个去重 ID，单批返回入口类型、公开空位数和资源状态；不创建报价、准入申请或租约。
- `GET /v1/share-market/me/client-rental-contexts?cursor=…`：仅本人有效租约／未结事项，按 Share 去重，每页 20。
- `GET /v1/share-market/shares/:id/seat-context?cursor=…&subscriptionId=…`：单 Share 公开车位和本人适用租约，每页 20 条历史，额外确保当前租约及显式选中的本人租约仍可查看。无公开挂牌也无本人记录时返回 404。

游标绑定本人账号和查询范围；指定他人 subscriptionId 返回 404。三个接口均 `private, no-store`。公开目录查询增加可选 Share ID 过滤，避免详情为一个 Share 加载全站全部车位。

前端批量注册已渲染卡片，不逐卡请求；15 秒前台轮询，后台页面暂停。按账号和登录 epoch 隔离状态与异步响应；卸载／切换账号取消读取。摘要、恢复列表、详情分别报告错误，读取失败不会被当成“资源已删除”或“没有租约”。读取和报价都有超时。

## 充值返回与提交恢复

首页使用 `/?marketShare=…&seatId=…&subscriptionId=…` 保存只读定位。充值返回白名单只允许这些根路径参数，拒绝外站、反斜线、重复参数、嵌套 returnTo 和会再次触发充值分流的参数。

当前标签页的 sessionStorage 仅保存账号＋登录 epoch 范围内的 Share/Seat ID、自动续租草稿；已提交但结果未知时，再保存原 quoteId 与 idempotencyKey。不保存凭证、报价、余额或他人租约。充值返回重新获取报价，必须再次明确确认，不自动购买。

提交前先保存原请求，使用同步互斥锁防止双击；25 秒超时、网络中断、5xx 或无效成功回执都视为结果未知。此时禁止修改请求、换报价或发起新购买，只能重试原请求；刷新页面／市场切换后仍可恢复，报价过期也不换幂等键。成功后的列表刷新失败不会误判为购买失败。登录或切换账号只恢复只读选择，不自动提交。

## 验证与发布

不新增数据库迁移或资金表，不变更旧租金、试用、退款、补偿、结算规则。与原有 Share 历史身份和按钮换行修复共同验证。

主要非 UI 回归入口：`npm run test:client-share-home`、`test:providers`、`test:share-market`、`test:client-market`、`test:market-wallet`、`test:model-routing`、`test:market-notifications`、`typecheck`、`build`；Rust 覆盖 `client_entries_tests`、`clients_home`、`router_ui_host`、`share_market::tests`、`share_market_termination::tests`、`client_chat::tests` 和 `schema::tests`。

本次按要求不截图、不执行浏览器／UI 自动化。既有浏览器脚本仅更新主机上下文 mock 并做 JavaScript 语法检查，未运行交互测试。上线前后端与前端需使用同一版本；远程环境部署不在本次变更范围内。
