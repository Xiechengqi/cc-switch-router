# Client Market 在线 Client 能力租用规划

本规划记录截至 2026-11-26 的在线 Client 租赁设计。卖家出租已验证 Client 的服务能力和网络出口，租户在独立工作区管理应用。第 5 节说明上架、选购和健康指标，第 6 节说明存量租约如何迁移，第 9.1 节记录五项调整的实现及兼容范围。第 11 节保留对应日期和版本的验收记录。

## 1. 已确认的产品决策

| 编号 | 决策 |
|---|---|
| D1 | Client Market 卖的是 Client 能力：服务、管理界面、Share、出口 IP 与地区，不卖主机 root。 |
| D2 | 开租关闭所有人的 Web Terminal，结束已有 PTY。 |
| D3 | 卖家在 Client Market 的「我的挂售 → 添加」选择自己的设备，也可从 Clients 卡片直接带入该机。Router 确认设备在线并具备独占管理能力后才接受上架。 |
| D4 | 租期内卖家**不能使用该 Client 的能力**：不能用自己的账号、Share 或流量，也包括卖家本机 CLI 经回环地址访问。 |
| D5 | 租期内控制台与应用管理权归租户。卖家的物理域名、本机管理接口和旧凭据均不能进入。 |
| D6 | 租户可在 Router Clients 页发起升级，在工作区重启、升级、回滚及设置自动升级策略。 |
| D7 | 从 SSH 托管切换到在线供给时采用空环境。已有在线租赁库通过迁移 0129 保留原租约、报价和回执，新租约直接采用独占管理。 |
| D8 | 只运行一个常驻进程（即 Client 本身），不在主机上另装 Agent 或 standby。 |
| D9 | 必须支持 NAT 后的主机。Client 本就通过反向隧道主动连 Router，这一点天然满足。 |
| D10 | 「选购 Client」统一展示已挂售的 Client，包含商家自己的设备。「我的挂售」单独管理供给。支持自租的设备对卖家本人成交价为零，对外挂牌价继续生效。 |
| D11 | 租户用下单的 Router 账号进入控制台。owner 的原密码保留在 Client 上，交还后要重新登录。卖家邮箱公开；租户邮箱只对该租约的卖家、租户本人和 Router 管理员可见，其他人看到「已租出」（2026-11-26 调整，见 [改进计划](client-market-improvement-plan.md) P1）。 |

## 2. 为什么放弃 SSH 托管

| 问题 | SSH 托管现状 | 在线 Client 租用 |
|---|---|---|
| Router 运营者的权限 | Router 用唯一一把 provision 私钥以 `root@ip` 登录所有主机，私钥是普通文件（原 provision SSH 模块）。Router 被入侵即等于所有主机 root。 | 只能经 HMAC 控制通道执行固定的租约指令和触发升级，不会以管理员身份进入 Client。 |
| NAT 后的主机 | 需要从外部连入 22 端口，不支持。子域名认领还要求请求来源 IP 等于主机 IP（`authorize_client_market_subdomain_claim`）。 | Client 主动外连，天然支持。 |
| 租户权限 | 托管安装刻意保留 Web 终端，Client 以 root 运行，租户在租期内有 root，可以留后门、读取下一任的数据。 | 租户只有工作区内的应用层权限，读不到 installation 私钥，无法冒充 Client 抢隧道。 |
| 主机上的进程 | 租期内 Client 加 standby 两个常驻进程，空闲期 standby 一个。 | 一个 Client 进程。 |
| 运维面 | SSH 上架、依赖安装、二进制预下载、远端防并发脚本、standby 部署、隔离复查、SSH 终端。 | 全部不再需要。 |

## 3. 目标模型

### 3.1 对象

| 对象 | 含义 | 取代 |
|---|---|---|
| 上架项（Listing） | 一个卖家已在线、owner 邮箱已验证、签名确认 `exclusiveManagement` 的在线 installation | `router_ssh_hosts`（IP、端口、SSH 主机密钥） |
| 租约（Lease） | 某个租户在一段时间内独占该 installation 的能力 | 每份在线租约都有独立的 ID 和别名 |
| 租户工作区（Workspace） | 租户在这个 Client 上的全部数据，与卖家数据隔离，租约结束时删除 | 旧模式整机重装、整个配置目录清掉 |
| 租户子域名 | 每份租约一个，在 Router 侧映射到该 installation 的租户工作区 | 旧模式每次租用注册一个全新的 installation 和子域名 |

### 3.2 角色与权限

| 能力 | 卖家（owner） | 租户 | Router 签名代入 |
|---|---|---|---|
| Web Terminal | 关闭 | 关闭 | 关闭 |
| 升级、重启、回滚、升级策略 | 无 | 当前买家，含自租 | 代买家提交签名升级 |
| installation 绑定、owner 邮箱、主机级设置 | 交还后恢复 | 无 | 平台只处理签名租约指令 |
| 工作区管理、Share、备份、聊天和推理 | 无 | 当前租户 | 按买家账号、别名和有效租约校验 |

Clients 卡片与抽屉显示完整 owner 邮箱。当前租户邮箱只对该租约的卖家、租户本人和 Router 管理员返回，其他查看者只看到「已租出」；租户 Share 的 owner 邮箱同样按此规则打码。释放后移除租户。[快照过滤](../src/store.rs)租户使用自己的 Router 登录，自租免付款并使用同一工作区规则。[代理鉴权](../src/proxy.rs) · [窗口票据](../src/store/client_web_sessions.rs)

## 4. cc-switch-server 改造

### 4.1 配置拆分为 installation 级和工作区级

| installation 级（始终属于卖家） | 工作区级（每份租约一份） |
|---|---|
| Router 身份：`installation_id`、Ed25519 私钥、`control_secret` | providers、accounts 及**独立的** `accounts.key` |
| owner 邮箱、owner 密码与会话 | shares、用量、请求审计、聊天记录 |
| 物理终端开关、出站代理、端口、owner 升级策略 | 租户会话、API token、租约自动升级策略 |
| 主机级日志 | 工作区日志、工作区备份 |

目录约定（示例）：

```
~/.cc-switch-server/                 # installation 级：server.json 中的身份、owner 认证、主机设置
~/.cc-switch-server/workspaces/owner/        # 卖家自己的工作区
~/.cc-switch-server/workspaces/lease-<id>/   # 租户工作区，租约结束时删除
~/.cc-switch-server/market-lease.json        # 上架状态、持久化指令序号和当前工作区授权
```

- 首次上架时备份现有数据。首次开租时将卖家数据迁入 `workspaces/owner/`，清理租户工作区后恢复到原路径。迁移可重入。
- **切换工作区通过重新执行同一二进制完成**。Server 持久化授权后结束当前运行时，以原启动参数载入目标工作区，重新建立 provider 密钥缓存、会话、Share 后台任务、SQLite 连接。installation 身份和卖家子域名持续有效，隧道自动重连。
- 租期内卖家工作区**冻结、不加载**。D4 靠这个机制保证，而不只是界面上隐藏。

### 4.2 身份与权限

Server 的 `resolve_web_admin_principal` 区分三个来源。

1. **LocalOwner** 在空闲安装上使用原密码。开租撤销该身份的浏览器会话并结束 PTY，租期内拒绝本机登录和管理请求。交还后用原密码重新登录。
2. **Router** 代入的物理身份在未释放租约期间同样被拒绝。签名平台控制通道继续处理同一租约的交付、回执和交还。
3. **LeaseTenant** 要求签名邮箱和请求域名同时与本机有效授权的 `tenantEmail,alias` 匹配。租户从自己的 Router 账号进入，不另设密码。租户的命令按 Web runtime contract 的 `workspaceAccess` 过滤，隧道接口投影本租约别名，创建和导入 Share 都按当前租户归属。

Router 控制面使用 installation HMAC 验证 `/_ctl/*`。未上架时，原有 Bearer 入口仍支持 Router 管理员协助，物理 Client 窗口票据仅签发给该 Client 的 owner。

此外：

- **本机回环也要拦截。** 租期内，127.0.0.1 上的代理请求同样必须持有租户工作区的 token。卖家本机 CLI 因此在租期内不能使用该 Client（D4，已接受）。
- **管理接口审计。** Server 的命令契约给每个 invoke 命令标注工作区权限。请求鉴权和 `/web-api/context` 使用同一分类。REST 入口单独按匹配路由授权；终端和 installation 级输入始终受主机策略约束。
- 备份与恢复只在当前工作区内生效；租户不能恢复 installation 级文件。

### 4.3 租户运维与交还

租户可从 Clients 卡片或个人租约详情升级 Client，也可在工作区调整本租约的升级策略。操作回执绑定租约，断线后重传。旧卖家协助与终端回执保留历史类型。退租先关闭租户入口，再处理第三方车位、审计回执与清理。Client 在本机保持锁定，直到收到 Router 已释放同一租约的签名确认。

### 4.4 上架和租期内锁定的主机级设置

| 设置 | 约束 | 原因 |
|---|---|---|
| 出站代理 | 租期内锁定 | 改动会改变租户买到的出口 IP 与地区 |
| owner 邮箱、Router 地址 | 上架期间锁定 | 上架项与收益归属的身份 |
| 终端开关 | 租约期间强制关闭 | 开租同时结束已有会话 |
| 重启、升级 | 租户可操作，按租约记录 | 停机计入探测记录（§5.7） |
| 下架 | Owner 可立即停止新订单，当前租约继续；本机同步独立推进 | 下架不改变租户权益 |
| 删除 installation 或改身份 | 有未完成租约、财务或清理义务时不允许 | 保护租约与清理隔离 |

### 4.5 租约指令 `/_ctl/market/*`

复用现有 HMAC 控制通道（Router [ctl_client.rs](../src/ctl_client.rs)，Server `src/api/market_workspace.rs`）。下表描述逻辑操作。写入通过 `/_ctl/market/command` 提交，带 installation 级单调递增的 `sequence`。重复投递须与已持久化的操作一致，过期序号直接拒绝。检查和读取分别通过 `/_ctl/market/check` 和 `/_ctl/market/status` 执行。实际字段和回执见 [PROTOCOL.md](../PROTOCOL.md)。

| 指令 | Client 行为 | 回执 |
|---|---|---|
| `/_ctl/market/check` | 按本次 challenge 检查设备并回调真实出口 | 版本与能力、工作区状态、资源快照。Share 挂售与车位占用由 Router 先查市场记录。 |
| `lease_begin{leaseId, buyerId, tenantEmail, expiresAt}` | 创建工作区、写入授权、冻结卖家工作区、切换到租户工作区 | 工作区就绪、授权已生效 |
| `lease_extend{leaseId, expiresAt}` | 更新本地到期时间 | 新到期时间 |
| `lease_end{leaseId}` | 撤销授权、吊销租户会话与 token、删除租户工作区、恢复卖家工作区 | 工作区已删除、授权已撤销 |
| `return` 与状态确认 | Client 确认工作区已清理，Router 落库释放后回传 `releasedLeaseId`，Client 才解锁本机管理 | 匹配本次租约的交还回执 |

- **本地到期处理**。授权到期立即停止租户访问。有待交付的访问记录或第三方车位时保留工作区，重连后完成核对。其余工作区经过五分钟续租宽限且已排空，可以离线清理。Router 收到与本次租约匹配的回执后才释放库存。
- Client 在注册或心跳中声明能力 `marketLeaseV1`。上架和新租约还需要签名的 `exclusiveManagement`。

### 4.6 出口与地区

- `installations` 已经记录 `last_seen_ip` 与 `country_code`，来自隧道来源 IP。
- 上游请求的实际出口可能不同，例如卖家配置了出站代理。`listing_check` 必须通过**上游实际使用的出站路径**访问 Router 的出口探测端点，由 Router 记录真实出口 IP 并判定地区。
- 两者不一致时，市场卡片使用实测出口国家的旗帜。上架项接口另提供 `egressDiffers`。未识别出口国家时使用未知地区标识。

### 4.7 资源快照

Client 在本进程内读取 cgroup 的有效 CPU、内存和工作区数据盘容量，经签名控制请求返回硬件快照。

## 5. Router 改造

### 5.1 卖家上架

「我的挂售 → 添加」和 Clients 卡片的「上架到 Client 市场」打开同一个弹窗。单台设备直接显示，多台可更换。从卡片进入时预选该机，原设备失效时由卖家重新选择。单栏弹窗按设备、出租方式、发布确认排列，正文可滚动，底部操作保持可见。[上架弹窗](../frontend/components/dashboard/client-market/client-listing-add-dialog.tsx)

| 卖家看到的状态 | 行为 |
|---|---|
| 检查设备 | Router 先查归属、激活与隧道、挂售资格、Share 占用，再经签名控制通道检查工作区、硬件和实际出口。返回首个阻塞及检查进度。仍有 Share 挂售或未释放车位时，弹窗显示原因和「管理 Share 挂售」，隐藏价格及确认项。返回自动复检，保留同一设备的草稿。 |
| 可以上架 | 展示实测出口国旗、CPU、内存、架构、24 小时在线率和运行时长。默认免费，每位租户的免费天数不限，也可设 1–365 天。选择按月收费后填写 0.01–1,000,000 美元的月租，精确到美分，可加 1–30 天免费试用。两种天数分别保存。 |
| 收费资格待处理 | 设备预检同时读取收费资格。首次收费的已验证用户可在弹窗内认领自己的供应商身份，验证成功后回到原草稿。邮箱未验证、收费暂停、身份冲突分别提供相应处理提示。 |
| 确认发布 | 卖家确认租户独占控制台并管理升级，租期内 Web 终端关闭。更换设备会重新检查并清除旧确认。提交时 Router 再核验。`locking` 显示「已提交上架，等待 Client 确认」。结果不明时先查现状，收到对应 Client 的签名回执后才显示为已挂售。 |

协议保留旧版响应字段。新界面用 `probeStatus` 区分跳过远端检查、未收到有效回执和已经检查，用 `blocker` 选择处理入口。预检使用短时 challenge，验证供应商身份与正式发布各自显式提交。[上架协议](../PROTOCOL.md#在线-client-market-购买与控制协议) · [Router 实现](../src/online_client_market.rs)

### 5.2 数据模型

- `online_client_listings` 以 `installation_id` 为主键，保存卖家、价格与试用条款、出口地区与 IP、上架状态、修订号、最近探测及硬件快照。
- `online_client_leases` 以租约 ID 标识每次出租。部分唯一索引 `online_client_exclusive` 保证每个 installation 同时只有一份未释放的租约。
- 退租、争议和审计使用不可变的 `terminationId`。退租记录中 `installation_id` 保存本次租约 ID，`host_id` 保存物理 installation ID。清理回执必须同时匹配这两个值。

### 5.3 租用流程

「选购客户端」按地区浏览，默认筛选可租设备，其中包含符合条件的自有挂售。卡片把出口国旗、规格、24 小时在线率、运行时长和价格放在一起，按钮为「租用」。他租在报价确认时简短提示主机所有者可读取磁盘。自租要求 Client 签名声明 `ownerRental`，对外月租照常展示在卖家的挂售管理中，自租报价为零，无钱包冻结、分佣、免费额度消耗或自动续费。两种租用都会独占同一设备，成交回执定位到「账户 → 我的租用」中的该笔租约。[选购卡片](../frontend/components/dashboard/client-market/online-shop.tsx) · [报价与成交](../src/online_client_market.rs)

```
下单（复核报价、钱包余额和卖家访问权限，买家确认租用）
 → lease_begin（ctl，等待回执）
 → 启用租户子域名映射
 → 租户凭现有租约票据自动登录
退租或到期
 → 立即删除租户路由映射、停止计费
 → 结束租户 Share（§5.5）
 → lease_end（ctl）
 → lease_status 核对工作区已删除
 → 结算 → 上架项恢复可租
```

- 开通等待最长十分钟。开通失败走撤权与解冻流程，买家可以查看原回执，待库存恢复后重新选购。
- 开启自动续费的付费月租，授权截止时间设为本期结束后再加 2 小时，避免每次续费都中断租户访问，也避免续费时 Client 短暂离线就丢失已付费的租约。续费失败或关闭自动续费时，合约状态变化会立即结束租约。
- 退租截止满 24 小时、第三方车位已结清但仍未收到清理回执时，Router 先按原规则退款或解冻买家余额（未开通的订单全额解冻，已开通的退还未使用部分），审计记为 `settled_without_cleanup`。上架项继续占用，直到 Client 交回与本租约匹配的清理回执；该回执不会重复结算。
- Client 离线时，Router 侧撤销照样立即生效，因为所有公网访问都必须经过 Router。磁盘清理等 Client 重连后由 ctl 或本地自治补做。**核对通过之前，上架项不能再租。**
- 租户在退租前可以导出工作区备份，复用现有备份接口，范围限定在工作区内。

### 5.4 入口与路由

- 每份租约分配一个租户子域名，Router 侧把它映射到同一 installation 的租户工作区。租户的 Share 使用 `{slug}--{租户子域名}`。
- 租期内卖家原子域名关闭控制台入口。
- 租约结束时删除映射，前租户手里的地址、票据和 token 立即失效。

### 5.5 Share 归属

- 租户创建或导入的 Share 使用当前工作区身份。Router 核对签名请求所属租约，将 Share 与 `leaseId` 绑定。租户是 Share 车位的收款人。[Share 归属](../src/online_client_market/shares.rs)
- 租约结束后，先撤销租户公网访问，再结清其挂售和第三方车位。Client 的清理回执与资金结算分别记录。原卖家恢复可租需要同时满足清理与结算条件。[Share Market 立即退租](share-market-immediate-termination.md)

### 5.6 Web 访问委托

[client_web_sessions.rs](../src/store/client_web_sessions.rs) 中的委托规则调整为：

| 访问者 | 规则 |
|---|---|
| 当前租户 | 从市场、账户或租户域名根地址进入独立标签页，以 Router 账号自动登录。旧 Client 回退到 Router 内嵌控制台。 |
| 卖家 | 空闲期正常登录，租期内只能在 Router 管理供给和退租事项 |
| Router 管理员和 Router owner | 继续不委托 |
| 市场页面的终端入口 | 租期内不显示入口，物理与租约请求均被拒绝。 |

浏览器经主域签发并用表单 POST 交接一次性票据。工作区窗口在到期前轮换短期 token，父会话失效后回主域重新验证。退租会撤销别名及窗口访问，买家可在确认退租之前直接下载工作区 ZIP。请求、响应、错误码及兼容握手见 [Client Web 窗口登录](../PROTOCOL.md#33-client-web-窗口登录)。

商店是买家的下单入口。自动续费选项变化或报价到期会在原弹窗重新报价，余额不足时可在原页充值。成交回执定位本次租约；响应丢失时，当前买家可用原报价 ID 重取同一回执。开通成功后通知买家，开通失败时展示冻结款状态。挂售条款可直接修改，只影响新订单；已成交租约保留原报价。

### 5.7 健康与结算

选购卡片、可租判定和租金结算分别回答设备是否稳定、现在能否交付、已交付服务如何计费。

| 用途 | 数据口径 |
|---|---|
| 选购与上架中的 24 小时在线率 | 最近 24 小时内有健康记录的分钟数 ÷ 有健康或不健康记录的分钟数。无观测显示「暂无观测」，覆盖不足半天时附样本分钟数。与 Clients 页共用隧道观测数据。 |
| 运行时长 | 从 installation 首次接入 Router 起算。当前系统这次开机的时间不参与计算。 |
| 当前可租 | 已挂售、没有未释放的租约、没有待完成的退租结算、具备租赁能力、签名确认独占管理，且最近十分钟工作区探测健康。自租还要确认自租能力。 |
| 服务结算与规格刷新 | Router 约每五分钟经签名通道探测服务，按 UTC 时段记账。硬件快照约每小时由 Router 主动请求。旧 standby 的每小时随机自报已退役。 |

- Router 定时检查已上架或租用中的 Client 的控制通道及工作区。每五分钟窗口内的调度带有抖动。
- 每 5 分钟记录一个探测时段。一个完整 UTC 小时含 12 个时段，其中 10 个成功即计为可用。首尾部分小时按相同比例计算。服务在某个时段中途开始或结束、而该时段带随机偏移的探测落在窗口之外时，该时段不计入分母，避免卖家因探测时机损失最后不满一小时的收益。
- 按小时结算的规则不变（不可用的小时不计供应商收益，相应费用退回平台余额），依据改为探测记录。
- 租户发起维护时的停机照常计入可用性。
- 卖家在「网络详情」可核对自己的完整出口 IP。其他人看到 IPv4 的前三段（如 `192.0.2.xx`）或 IPv6 的 `/64` 网段，IPv4 映射地址按 IPv4 处理，无法解析的地址留空。非卖家看到的设备名使用 `client-<installation 前 8 个字母或数字>`。[地址处理](../src/store.rs) · [网络详情](../frontend/components/dashboard/client-market/online-shop.tsx)
- 当前页面只请求在线率、观测覆盖和运行时长等选购数据。旧版页面仍可通过原列表请求取得过去 24 个完整 UTC 小时的可用小时数，详见 §9.1。[列表 API](../src/online_client_market.rs) · [市场探测与对账](../src/online_client_market/reconcile.rs)

## 6. 环境迁移与验收计划

新环境可从空库初始化。已有在线租赁库经 0129 将存量租约标为管理版本 1，0130 更新供给视图。现有租约 ID、资金和历史事件继续有效。原买家可重取已成交报价的回执，未成交的旧报价需要重新获取。[租约迁移](../schema/0129_client_rental_management.sql) · [供给视图](../schema/0130_online_client_supply_availability.sql)

版本 1 的有效租约保留买家的工作区，缺少独立窗口交接能力时回退至内嵌窗口。Router 关闭物理公网窗口，旧宿主程序仍按原规则处理本机访问。Clients 卡片提示待迁移，抽屉和买家租约详情说明两种处理方式。需要继续履约时由 owner 在宿主上更新 Client，也可先退租再由 owner 升级。原位更新后，Router 须收到序号一致的签名回执，确认独占能力、同一租约、别名、租户邮箱、运行工作区及有效授权期限，回执若带买家 ID 也须匹配。正在封锁、切换或交还的工作区会继续等待核对。确认后原租约转为版本 2，有升级能力的 Client 自动向租户显示升级入口。[状态核对](../src/online_client_market/reconcile.rs) · [买家租约详情](../frontend/components/dashboard/client-market/rental-records.tsx)

以下步骤用于环境验收。本轮只做静态检查，未执行环境验收。

1. 在新环境为 Router 准备独立数据目录、空数据库、域名、TLS 和 owner 设置，按正常启动方式初始化。Router 的入站 SSH host key 用于 Client 反向隧道。
2. 在另一个空数据目录初始化 Server，配置同一 Router、Client 子域名和卖家身份，验证 owner 并建立反向隧道。Client 进程直接处理资源探测。
3. 分别从「我的挂售 → 添加」和 Clients 卡片进入。检查 Share 占用、处理后返回、免费和月租切换、收费资格验证、发布待确认及签名回执后的库存。检查卡片的地区旗帜、CPU、内存、架构、在线率与运行时长。
4. 用第二个账户报价并确认租用，检查独立租户工作区、租户升级权限及卖家入口和所有网页终端的关闭状态。在 Clients 卡片和抽屉核对买卖双方及匿名查看的完整邮箱。若验证第三方车位，让第三个账户购买租户工作区内的 Share 车位。
5. 租户退租后核对旧票据立即失效。第三方车位须先撤权结算，卖家工作区恢复并完成本次清理回执与待处理结算后，卡片应自动恢复可租。最后由卖家下架并重新上架，记录公网链路的实际验收结果。

## 7. 信任关系

| 一方 | 能做什么 | 与 SSH 模式比 |
|---|---|---|
| 卖家 | 持有主机 SSH/root 权限，租期内无 Web 控制台或终端权限 | 系统权限仍由主机所有者持有 |
| 租户 | 只有工作区内的应用层权限，没有终端 | 更安全：读不到 installation 私钥，无法冒充 Client 抢隧道或留后门 |
| 前租户 | 地址、票据和 token 随映射删除失效；工作区随租约结束删除 | 更安全 |
| Router 运营者 | 只能执行固定的租约指令和触发升级（升级来源固定为 GitHub release） | 从“所有主机长期 root”缩小到固定指令 |

Router 运营者与 release 发布者目前是同一方，因此本方案不防运营者本人作恶。它收窄的是 Router 被入侵后的影响面。

## 8. 边界情况

| 场景 | 处理 |
|---|---|
| 租期内 Client 离线（卖家关机、网络或进程故障） | 探测失败的小时不计供应商收益，费用退回；长时间离线触发现有服务中断与退租规则 |
| 到期或退租时 Client 离线 | Router 侧立即撤销；重连后执行 `lease_end` 或本地自治清理；核对前不能再租。离线满 24 小时先完成买家退款或解冻（§5.3） |
| 卖家 Client 下单后再未上线 | 未开通的订单 10 分钟后结束；24 小时后全额解冻买家余额，上架项保持占用 |
| `lease_begin` 回执超时 | 先用 `lease_status` 判定是否已生效；未生效则不扣费并允许重试；已生效则继续后续步骤 |
| 卖家在租期内下架 | 立即停止新订单，当前租约继续，本机下架在租约收尾后同步 |
| 卖家在租期内删除或改 owner 邮箱 | 不允许 |
| 卖家在租期内改出站代理 | 锁定 |
| 租户升级后出现问题 | 自升级流程已有健康检查与回滚；停机计入探测记录 |
| 卖家想绕过冻结使用租户账号 | 回环同样需要租户 token；root 直接读盘只能靠告知与信誉约束 |
| 卖家 Client 版本过旧 | 缺少独占管理能力时暂停新上架、新报价和成交。存量租约在签名确认升级前显示待迁移 |
| 租户 Share 上仍有第三方车位 | 先走 Share 立即退租与结算，再下发 `lease_end` |
| 卖家需要收回机器 | 默认提前 24 小时通知（已付租期更早结束则在租期结束时收回；未开通的订单立即取消），期间自动续费关闭、「试用后继续」改为仅试用、租户退租不扣赔付，取消收回后恢复原来的设置；卖家的说明对租户可见并写入退租记录；租户违规时可立即收回，必须写明原因。两种方式都按卖家责任结算，租户未使用的租金全额退回。可选「以后拒绝这个租户」「收回后下架」。[卖家操作](../src/online_client_market/seller.rs) · [到期执行](../src/online_client_market/reconcile.rs) |
| 租期内出口 IP 变化 | Router 每小时重新验证真实出口；IP 变化时记入租约事件并通知租户（只显示网段与国家）。[出口复核](../src/online_client_market/reconcile.rs) |
| 卖家想暂停接新订单 | 「暂停接单」不影响当前租约；商店隐藏，报价和成交被拒绝，恢复后可再租。修改条款只影响之后的新订单 |

## 9. 分期与验收

| 阶段 | 内容 | 验收要点 |
|---|---|---|
| P0 供给切换 | 建立在线 Client 上架入口 | 市场仅展示在线 Client 供给 |
| P1 Client 端 | 配置拆分与数据迁移；工作区与切换；三类身份；管理接口审计与标注；独占管理与租约操作回执；`/_ctl/market/*`；本地自治清理；出口探测；资源快照；`marketLeaseV1` | 卖家数据在租期内不可用、租后完整恢复；租户看不到 installation 级数据；回环拦截生效；指令幂等、过期序号被拒绝 |
| P2 Router 端 | 上架项与前置检查；租约流程；租户子域名映射；Share 按工作区归属与租后结束；Web 委托调整；主动探测与结算改造 | 租户建的 Share 收益归租户；租后旧入口立即失效；离线到期能补清理；核对前不能再租 |
| P3 前端 | 上架弹窗、选购卡片、租户控制台与升级、完整租户身份、退租前备份 | Share 阻塞时只有处理入口，收费验证保留草稿，自租报价为零，租户通过自己的 Router 账户进入工作区 |
| P4 空环境切换 | 移除 SSH 托管入口、出站 provision 密钥、远端任务和独立 standby | 空数据库可初始化，在线工作区、主动探测、退租和反向隧道工作正常 |

<a id="client-market-design-gaps"></a>

### 9.1 五项调整的实施结果（2026-11-26）

| 项目 | 当前行为及兼容范围 |
|---|---|
| 存量租约 | Clients 与买家租约详情识别待迁移的版本 1 租约。原位更新以有效签名状态为准，保留原租约、别名和账务。首次迁移与交还后升级的路径见第 6 节。[租约核对](../src/online_client_market/reconcile.rs) · [租户升级](../src/online_client_market/upgrades.rs) |
| 可租供给 | 商店、报价及首次成交、卖家库存、地区统计、排名空闲数和准入申请统一使用供给视图的 `availability`。缺少有效检查快照的已挂牌设备继续接受签名探测。总挂售包含已挂牌的占用或待升级设备，可租数只计 `available`。自租单独核对 `ownerRental`，第三方买家的准入和收费条件仍在交易时核验。[供给视图](../schema/0131_client_market_seller_controls.sql) · [报价与库存](../src/online_client_market.rs) · [地区统计](../src/market_providers.rs) · [排名](../src/market_provider_rank.rs) · [准入](../src/market_access.rs) |
| 出口地址 | 服务端按地址族生成公开网段，卖家仍能核对完整出口；租户在租期内可看到完整出口 IP。商店卡片以出口国家和网段为首要信息。租户邮箱不再公开（D11）。[地址处理](../src/store.rs) · [出口展示](../frontend/components/dashboard/client-market/online-shop.tsx) |
| 不可租原因 | 最近十分钟有明确隧道断开记录才显示离线。签名回执可区分升级中和工作区切换中。控制请求失败、缺少观测或观测过期时，已具备独占管理的空闲设备显示暂不可租。[探测与对账](../src/online_client_market/reconcile.rs) · [前端状态](../frontend/components/dashboard/client-market/host-utils.ts) |
| 旧健康展示 | 已删除停用的 standby 健康组件辅助代码和词库。当前页面请求 `includeAvailableHours24=false`，跳过可用小时聚合。旧版商店还读取 `availableHours24`，原 URL 继续返回真实结果，一次查询按设备和小时汇总。五分钟履约样本仍用于结算。[列表请求](../frontend/lib/online-client-market.ts) · [兼容查询](../src/online_client_market.rs) |

本轮未运行测试或部署。行为验收条件见 [Client Market 用例 H-01 至 H-32](ui-test-plan.md#7-client-market-在线供给h)。

### 9.2 改进计划的实施（2026-11-26）

租户邮箱不公开、卖家收回、公开标题与修改条款、暂停接单、Client 仅试用与精简结账、以出口为中心的商店（30 天在线率、按接入时长排序、搜索与分页、详情抽屉）、「我租的 Client」与开通引导、计费小时展示和用词统一已按 [Client Market 改进实施计划](client-market-improvement-plan.md) 实施。数据库变更见 [迁移 0131](../schema/0131_client_market_seller_controls.sql)，供给视图在其中增加 `paused`。本轮同样未运行测试或部署。

## 10. 关键代码位置

| 主题 | 位置 |
|---|---|
| Router 签名身份与租户授权校验 | cc-switch-server `src/api/session.rs`（`resolve_web_admin_principal`） |
| HMAC 控制通道 | Router [ctl_client.rs](../src/ctl_client.rs)；Server `src/api/market_workspace.rs` |
| Share 工作区绑定与收益归属 | [shares.rs](../src/online_client_market/shares.rs)（`bind_share`、`share_owner`） |
| 市场 Client 的隔离与委托 | [proxy.rs](../src/proxy.rs)（`online_client_market::listed`）；[client_web_sessions.rs](../src/store/client_web_sessions.rs) |
| 租约唯一索引 | [迁移 127](../schema/0127_online_client_leasing.sql)（`online_client_exclusive`） |
| Share 托管授权（可参照） | [share_market.rs](../src/share_market.rs)（`managedGrant`、`try_apply_dispatched_edit_via_ctl`） |
| 资源采集 | cc-switch-server `src/workspace_hardware.rs` |


## 11. 历史实现与运维记录

以下为当时版本的验收记录。2026-11-26 的独占管理规则取代下列历史卖家协助流程，当次测试结果仅适用于其记录的版本。

- 在线租约放在迁移 127 的独立表中，物理 installation 和租约使用各自的身份。平台钱包、准入、退租预览、赔付和争议共用既有服务。公开结算证据只取本租约的探测时段。
- Server 首次上架会做一致性备份。在持有 installation 目录锁的启动阶段，存在租户授权就把卖家私有存储原子移至 `workspaces/owner/`，再加载 `workspaces/lease-<uuid>/`。恢复卖家时将原文件移回原路径，使离线维护工具和既有备份继续可用。逐项重命名与目录同步可重入；同时存在两份文件时停止恢复并保留两份。当前工作区以 installation 级 `market-lease.json` 为准，进程切换使用原参数重新执行同一二进制。
- 买家通过绑定租约的现有 Router 登录票据进入工作区。回环 CLI 使用独立可轮换令牌并指定 `X-Workspace-Share`。卖家的协助视图通过固定字段投影 provider 健康、HTTP 状态和访问事件，终端和维护操作要求本机 owner 身份。租户可导出备份；曾经承载第三方托管授权的工作区保留结算证据，禁止回滚旧快照。
- 按 UTC 自然小时结算。每小时统计成功的不同五分钟时段，首尾部分使用相同的 5/6 门槛，完整小时以 10/12 计。结束时先停止路由和计费，已知的第三方订阅按父租约截止时间处理，退租记录使用独立 terminationId。
- P4 以新环境为发布边界。仓库内验证使用本地构建、隔离测试和空库迁移。实际部署与公网验收由用户手动完成。
- 自动结束租约先持久化截止时间并撤销公网入口。钱包暂时不可用时按原截止时间重试，结算挂起的 installation 不接新单。付费报价展示提前退租赔付比例，并冻结供应商首期佣金条款。首次提交时校验报价对应的试用额度版本和剩余秒数。试用额度已重置的买家需重新报价，已成交买家重试原报价会取回同一租约。
- 协助页展示资源、最近心跳和 provider 健康。卖家凭本机 owner 身份调整终端及升级策略，设置写回 installation。市场上架期间，Router 发起升级使用单独的签名控制端点，自动升级和委托升级同样产生租户可见的记录。
- P0–P3 当时的本地联测使用独立 Router、Server 进程、临时数据库和真实反向隧道。已验证上架、出口及硬件读取、同一报价重复提交、切换工作区、租户票据和卖家协助。免费和付费均覆盖租户创建 Share、第三方购买车位、父租约退租的完整流程。退租后旧票据立即失效。测试钱包验证车位收款方为租户、第三方取得退款、每笔分录平衡。收到最终消费回执并完成结算后，Server 清除租户工作区、恢复卖家数据，Client 恢复可租。独立回归覆盖 UTC 小时的 10/12 与 9/12 两种结算结果、同秒开通和退租、历史回执补收。
- 等待清理的租约分批轮询。即使首批 100 台 Client 持续离线，后续租约的第三方车位也会进入退租处理。Server 升级回滚在健康检查截止时再次读取替换进程的退出状态，运维记录能区分进程退出和健康检查超时。
- P0–P3 在 2026-11-24 的本地验收中，Router Rust 全量分片 2,106 项通过、1 项忽略，Client Market 前端单测 65 项通过，生产构建及结账、退租、审计浏览器回归均通过。Server Rust 4,760 项通过、5 项忽略，Web 452 项通过，静态检查通过。离线发布检查为 `contract_verified`、0 个内部阻塞。真实 Provider 凭据和生产部署仍待验证。

### P4 空环境切换的本地验收（2026-11-24）

| 范围 | 实际结果 |
|---|---|
| 空库 | 迁移 127 成功，重复初始化通过。最终 295 张表的增删改查、13 个视图的查询均通过 SQL 编译检查，外键引用的目标表全部存在。旧 SSH 主机、作业、终端授权和 standby 上报表均已移除。 |
| Router | 常规二进制构建通过。Rust 测试清单 1,985 项逐项核对，全部通过。覆盖同一 installation 排他租用、试用重置后的拒单、佣金从报价传递到首期、退租清理和卖家历史隔离。 |
| Router 前端 | TypeScript 单测 602 项、类型检查和生产构建通过。结账、退租、Owner 审计和 C2C 四组本地模拟浏览器流程通过。结账覆盖 409 后重新报价，以及成功回执丢失、报价过期后取回同一租约。 |
| Server | Rust 4,752 项通过、5 项按测试标记忽略。Web 在 Node 22 下 452 项通过，类型检查通过。Provider 审计套件 2,205 项通过。 |

### 工作区交付、买卖双方路径与兼容性验收（2026-11-25）

- 使用空 Router 库、独立 Server 目录及反向 SSH 隧道，验证了只读上架预检、付费成交、同一报价重取原租约和独占。租户创建的 Provider 和 Share 归属当前工作区。导入含其他 owner 邮箱的 Share 时，Server 将归属重设为当前租户。第三个账户实际购买了 Share 车位。结束父租约后第三方撤权并收到退款，所有测试钱包分录平衡。卖家的原始数据恢复后库存重新可租，卖家可下架。
- 使用两个二进制内置的 Web 页面，走通卖家发布、买家下单、从市场和账户页开新窗口、刷新、单窗口退出、ZIP 备份、结算退租及旧窗口撤权。同一租户窗口超过 10 分钟后成功轮换凭据，并读取到已认证的租户上下文。基于 `28b7649` 的旧 Server 本地构建也验证了从别名自动回退到已登录的内嵌工作区。
- 卖家已打开的本机控制台收到上架提示，切换进程后用原密码重新登录并进入协助视图。通过 Router 内嵌窗口进入时，先验证本机密码，再连接终端。浏览器确认了终端行列数，保存并恢复了一项主机设置。
- 本地浏览器回归可用 `frontend/` 下的 `npm run test:client-checkout-browser`、`npm run test:client-termination-browser` 和 `npm run test:client-rental-audit-browser` 重跑。双进程浏览器用 `npm run test:client-workspace-browser`，从标准输入读取一次性夹具 JSON。字段为 `origin`、`installationId`、`sellerEmail`、`sellerToken`、`sellerPassword`、`buyerEmail`、`buyerToken`。夹具使用可发布的测试 Client、已登录的两个独立账户。可选 `ownerLocalOrigin` 和 `sellerPassword` 用于验证本机入口在租期内关闭、交还后原密码登录。设置 `WORKSPACE_LONG_SESSION=1` 可验证跨 10 分钟的工作区会话。[浏览器验收脚本](../frontend/scripts/client-workspace-browser-test.mjs)

| 本地回归 | 结果 |
|---|---|
| 数据兼容 | 在 0127 库中保留一份上架、活跃租约和旧访问回执，再执行 0128。原数据与外键检查通过，原回执次数为 1，待确认批次初始化为空。 |
| Router | Rust 1,986 项通过。前端 TypeScript 单测 579 项、国旗资源测试 5 项通过，类型检查和生产构建通过。 |
| Server | Rust 共 4,783 项通过、5 项按原标记忽略，含 139 项 API 契约测试。Web 单测 454 项通过，类型检查和生产构建通过。 |
| 契约与审计 | Server 审计测试 2,207 项通过。262 条 Web 命令均有工作区权限分类，Provider 覆盖、UI 矩阵及写入路径审计通过。 |
| 浏览器专项 | 结账、续费重报价、原页充值、过期及回执重放通过。退租争议和管理员审计通过中英双语、键盘及多宽度回归。 |
| Server 本地发布检查 | `RUN_TESTS=1 RUN_REAL=0 scripts/release-readiness.sh` 返回 `contract_verified`。本地启动、密码登录、Provider 与 Share 冒烟通过，失败 0 项、内部阻塞 0 项。 |

公网域名与 TLS、生产部署、真实上游 Provider/OAuth 调用、安装新版本与回滚二进制仍待环境验收。

## 2026-11-27 身份恢复与离线下架修订

普通离线清理保留持久身份和域名归属，原密钥恢复原 ID。Owner 下架不依赖原机恢复；市场状态、本机同步、租约与资金分别推进。旧 unlocking 迁移为已下架，本机同步等待重连；在租保护不能只判断挂售状态。权威补充见 [身份恢复与离线下架](client-identity-recovery-and-offline-unlisting.md)。
