# Client Market 在线 Client 能力租用规划

**部署基线为全新 Router 数据库和全新 Server 数据目录。** Client Market 出租已在线、owner 已验证的 Client 服务能力及出口 IP。租户使用独立工作区。本地双进程和真实浏览器的验收记录见第 11 节，公网部署步骤见第 6 节。

## 1. 已确认的产品决策

| 编号 | 决策 |
|---|---|
| D1 | Client Market 卖的是 Client 能力：服务、管理界面、Share、出口 IP 与地区，不卖主机 root。 |
| D2 | 租户**没有终端**。终端只对 Client owner（卖家）开放。 |
| D3 | “添加主机”只能从卖家已在线的 Client 发起；彻底删除“SSH 方式添加主机”。 |
| D4 | 租期内卖家**不能使用该 Client 的能力**：不能用自己的账号、Share 或流量，也包括卖家本机 CLI 经回环地址访问。 |
| D5 | 租期内卖家**可以继续登录和运维**，以便协助租户定位问题。协助视图**默认开放**给卖家，不需要租户开启。 |
| D6 | 卖家在租期内重启或升级 Client 时，**只通知租户**，不需要租户同意。 |
| D7 | Router、Server 均从空环境安装，直接移除旧 SSH 托管。现网旧主机、租约、私钥和数据库不参与本次发布。 |
| D8 | 只运行一个常驻进程（即 Client 本身），不在主机上另装 Agent 或 standby。 |
| D9 | 必须支持 NAT 后的主机。Client 本就通过反向隧道主动连 Router，这一点天然满足。 |

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
| 上架项（Listing） | 一个卖家已在线、owner 邮箱已验证、具备 `marketLeaseV1` 能力的 installation | `router_ssh_hosts`（IP、端口、SSH 主机密钥） |
| 租约（Lease） | 某个租户在一段时间内独占该 installation 的能力 | 每份在线租约都有独立的 ID 和别名 |
| 租户工作区（Workspace） | 租户在这个 Client 上的全部数据，与卖家数据隔离，租约结束时删除 | 旧模式整机重装、整个配置目录清掉 |
| 租户子域名 | 每份租约一个，在 Router 侧映射到该 installation 的租户工作区 | 旧模式每次租用注册一个全新的 installation 和子域名 |

### 3.2 角色与权限

| 能力 | 卖家（owner） | 租户 | Router 签名代入 |
|---|---|---|---|
| 终端 | ✅ | ❌ | ❌ |
| 升级、重启、回滚、升级策略 | ✅，租期内通知租户 | ❌ | 只能触发升级 |
| 主机级设置（出站代理、端口、Router 地址、owner 邮箱、终端开关） | ✅，但部分项在上架或租期内锁定（§4.4） | ❌ | ❌ |
| 工作区管理（provider、账号、Share、备份、聊天） | 租期内只有只读协助视图 | ✅ | ❌ |
| 使用代理能力（Share 路由、本机回环 CLI） | ❌，卖家工作区冻结 | ✅ | — |
| 租约指令 `/_ctl/market/*` | ❌ | ❌ | ✅ |

Router 根据 installation 的上架状态限制管理员委托。物理 Client 的终端要求卖家的本机 owner 凭据。租户票据绑定租约 ID、别名和有效期。[代理鉴权](../src/proxy.rs) · [窗口票据](../src/store/client_web_sessions.rs)

## 4. cc-switch-server 改造

### 4.1 配置拆分为 installation 级和工作区级

| installation 级（始终属于卖家） | 工作区级（每份租约一份） |
|---|---|
| Router 身份：`installation_id`、Ed25519 私钥、`control_secret` | providers、accounts 及**独立的** `accounts.key` |
| owner 邮箱、owner 密码与会话 | shares、用量、请求审计、聊天记录 |
| 终端开关、出站代理、端口、升级策略 | 租户会话、API token |
| 主机级日志 | 工作区日志、工作区备份 |

目录约定（示例）：

```
~/.cc-switch-server/                 # installation 级：server.json 中的身份、owner 认证、主机设置
~/.cc-switch-server/workspaces/owner/        # 卖家自己的工作区
~/.cc-switch-server/workspaces/lease-<id>/   # 租户工作区，租约结束时删除
~/.cc-switch-server/market-lease.json        # 上架状态、持久化指令序号和当前工作区授权
```

- 首次上架时备份现有数据。首次开租时将卖家数据迁入 `workspaces/owner/`，清理租户工作区后恢复到原路径。迁移可重入。
- **切换工作区通过重新执行同一二进制完成**。Server 持久化授权后结束当前运行时，以原启动参数载入目标工作区，重新建立 provider 密钥缓存、会话、Share 后台任务、SQLite 连接和终端会话。installation 身份和卖家子域名持续有效，隧道自动重连。
- 租期内卖家工作区**冻结、不加载**。D4 靠这个机制保证，而不只是界面上隐藏。

### 4.2 身份与权限

Server 的 `resolve_web_admin_principal` 区分三个来源。

1. **LocalOwner** 包括本机密码会话、由本机验证的 owner 登录会话和安装方的管理凭据。租期内进入协助视图。本机已验证的 owner 可以执行维护。直连 `/api/auth/login` 会话保存在进程内，开租切换进程后，卖家用原本机密码重新登录即可进入协助视图。
2. **Router** 表示经 Router 验签的委托身份。上架后只有与 installation owner 匹配的卖家可通过物理子域名进入协助视图。在 Router 窗口中，终端、重启、升级、回滚和主机设置写入均额外验证 Client 独立签发的 owner proof。界面通过维护探测后解锁，凭据失效时重新锁定。
3. **LeaseTenant** 要求签名邮箱和请求域名同时与本机有效授权的 `tenantEmail,alias` 匹配。租户从自己的 Router 账号进入，不另设密码。租户的命令按 Web runtime contract 的 `workspaceAccess` 过滤，隧道接口投影本租约别名，创建和导入 Share 都按当前租户归属。

Router 控制面使用 installation HMAC 验证 `/_ctl/*`。未上架时，原有 Bearer 入口仍支持 Router 管理员协助，物理 Client 窗口票据仅签发给该 Client 的 owner。

此外：

- **本机回环也要拦截。** 租期内，127.0.0.1 上的代理请求同样必须持有租户工作区的 token。卖家本机 CLI 因此在租期内不能使用该 Client（D4，已接受）。
- **管理接口审计。** Server 的命令契约给每个 invoke 命令标注工作区权限。请求鉴权和 `/web-api/context` 使用同一分类。REST 入口单独按匹配路由授权；终端和 installation 级输入始终受主机策略约束。
- 备份与恢复只在当前工作区内生效；租户不能恢复 installation 级文件。

### 4.3 卖家协助模式（默认开放）

- 租期内卖家登录 Client 后自动进入**明确标注的协助模式**，不需要租户授权（D5）。
- 卖家能看到：租户工作区里各 provider 的健康状况与报错、请求审计和日志（token 与密钥脱敏）、版本、资源和连通性。
- 卖家看不到：明文密钥、OAuth 令牌、API token；也不能修改租户配置，不能使用租户账号。
- 协助访问与终端连接按租约、类型、访问会话去重。重启、升级、回滚及主机设置写入按操作记录。离线积压超过一批时，Client 按类型合并尚未上报的记录并保留次数。已上报记录保持不可变，Router 按事件 ID 去重并向租户通知。
- 必须向租户明确告知：卖家有主机 root，技术上能直接读盘。协助视图的脱敏只约束界面，挡不住 root。告知位置为租用确认页和租户控制台。

### 4.4 上架和租期内锁定的主机级设置

| 设置 | 约束 | 原因 |
|---|---|---|
| 出站代理 | 租期内锁定 | 改动会改变租户买到的出口 IP 与地区 |
| owner 邮箱、Router 地址 | 上架期间锁定 | 上架项与收益归属的身份 |
| 终端开关 | 卖家可改，不影响租户 | 终端只对 owner 开放 |
| 重启、升级 | 允许，通知租户 | D6；停机计入探测记录（§5.7） |
| 下架、删除 installation | 租期内不允许，只能走退租流程 | 保护租约 |

### 4.5 租约指令 `/_ctl/market/*`

复用现有 HMAC 控制通道（Router [ctl_client.rs](../src/ctl_client.rs)，Server `src/api/market_workspace.rs`）。下表描述逻辑操作。写入通过 `/_ctl/market/command` 提交，带 installation 级单调递增的 `sequence`。重复投递须与已持久化的操作一致，过期序号直接拒绝。检查和读取分别通过 `/_ctl/market/check` 和 `/_ctl/market/status` 执行。实际字段和回执见 [PROTOCOL.md](../PROTOCOL.md)。

| 指令 | Client 行为 | 回执 |
|---|---|---|
| `listing_check` | 自检，不改状态 | 版本与能力、当前工作区、是否存在进行中的 Share 挂售或车位、出口探测结果、资源快照 |
| `lease_begin{leaseId, tenantEmail, expiresAt}` | 创建工作区、写入授权、冻结卖家工作区、切换到租户工作区 | 工作区就绪、授权已生效 |
| `lease_extend{leaseId, expiresAt}` | 更新本地到期时间 | 新到期时间 |
| `lease_end{leaseId}` | 撤销授权、吊销租户会话与 token、删除租户工作区、恢复卖家工作区 | 工作区已删除、授权已撤销 |
| `lease_status` | 只读 | 当前工作区、授权与清理状态，用于核对 |

- **本地到期处理**。授权到期立即停止租户访问。有待交付的访问记录或第三方车位时保留工作区，重连后完成核对。其余工作区经过五分钟续租宽限且已排空，可以离线清理。Router 收到与本次租约匹配的回执后才释放库存。
- Client 在注册或心跳中声明能力 `marketLeaseV1`。不具备该能力的版本不能上架。

### 4.6 出口与地区

- `installations` 已经记录 `last_seen_ip` 与 `country_code`，来自隧道来源 IP。
- 上游请求的实际出口可能不同，例如卖家配置了出站代理。`listing_check` 必须通过**上游实际使用的出站路径**访问 Router 的出口探测端点，由 Router 记录真实出口 IP 并判定地区。
- 两者不一致时，以真实出口为准，并在上架项上标注。

### 4.7 资源快照

Client 在本进程内读取 cgroup 的有效 CPU、内存和工作区数据盘容量，经签名控制请求返回硬件快照。

## 5. Router 改造

### 5.1 上架：只从在线 Client 发起

入口在 Clients 页的 Client 卡片“上架到 Client 市场”，只对已验证的 owner 本人显示。前置条件：

1. owner 邮箱已验证，Client 在线，具备 `marketLeaseV1`。
2. 卖家工作区的 Share Market 挂售、车位和订阅都已结束。上架后禁止卖家工作区新建挂售。租户可在租约生效后为自己工作区的 Share 上架车位，收益归租户。
3. `listing_check` 通过，出口探测已完成。
4. 卖家确认告知事项：租期内能力由租户独占，卖家数据冻结、本机 CLI 不可用，卖家的终端与协助操作对租户可见。

### 5.2 数据模型

- `online_client_listings` 以 `installation_id` 为主键，保存卖家、价格与试用条款、出口地区与 IP、上架状态、修订号、最近探测及硬件快照。
- `online_client_leases` 以租约 ID 标识每次出租。部分唯一索引 `online_client_exclusive` 保证每个 installation 同时只有一份未释放的租约。
- 退租、争议和审计使用不可变的 `terminationId`。退租记录中 `installation_id` 保存本次租约 ID，`host_id` 保存物理 installation ID。清理回执必须同时匹配这两个值。

### 5.3 租用流程

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
- 卖家原子域名在租期内只用于 owner 登录和协助视图，不提供代理能力。
- 租约结束时删除映射，前租户手里的地址、票据和 token 立即失效。

### 5.5 Share 归属

- 租户创建或导入的 Share 使用当前工作区身份。Router 核对签名请求所属租约，将 Share 与 `leaseId` 绑定。租户是 Share 车位的收款人。[Share 归属](../src/online_client_market/shares.rs)
- 租约结束后，先撤销租户公网访问，再结清其挂售和第三方车位。Client 的清理回执与资金结算分别记录。原卖家恢复可租需要同时满足清理与结算条件。[Share Market 立即退租](share-market-immediate-termination.md)

### 5.6 Web 访问委托

[client_web_sessions.rs](../src/store/client_web_sessions.rs) 中的委托规则调整为：

| 访问者 | 规则 |
|---|---|
| 当前租户 | 从市场、账户或租户域名根地址进入独立标签页，以 Router 账号自动登录。旧 Client 回退到 Router 内嵌控制台。 |
| 卖家 | 空闲期正常登录；租期内进入协助模式 |
| Router 管理员和 Router owner | 继续不委托 |
| 市场页面的终端入口 | 只对 Client owner 开放，入口经过 Server 的独立 owner 认证。 |

浏览器经主域签发并用表单 POST 交接一次性票据。工作区窗口在到期前轮换短期 token，父会话失效后回主域重新验证。退租会撤销别名及窗口访问，买家可在确认退租之前直接下载工作区 ZIP。请求、响应、错误码及兼容握手见 [Client Web 窗口登录](../PROTOCOL.md#33-client-web-窗口登录)。

商店是买家的下单入口。自动续费选项变化或报价到期会在原弹窗重新报价，余额不足时可在原页充值。成交回执定位本次租约；响应丢失时，当前买家可用原报价 ID 重取同一回执。开通成功后通知买家，开通失败时展示冻结款状态。当前版本通过下架后重挂修改价格，已成交租约保留原报价。

### 5.7 健康与结算

所售能力只能经由 Router 访问，因此可用性由 **Router 主动观测**，不依赖主机自报：

- Router 定时向每个已上架或租用中的 Client 发送带 nonce 的 ctl 探测，同时参考隧道在线状态和 installation 心跳。
- 每 5 分钟记录一个探测时段。一个完整 UTC 小时含 12 个时段，其中 10 个成功即计为可用。首尾部分小时按相同比例计算。服务在某个时段中途开始或结束、而该时段带随机偏移的探测落在窗口之外时，该时段不计入分母，避免卖家因探测时机损失最后不满一小时的收益。
- 按小时结算的规则不变（不可用的小时不计供应商收益，相应费用退回平台余额），依据改为探测记录。
- 卖家重启或升级造成的停机如实计入探测记录（D6 只免除了征得同意，不免除可用性责任）。
- 买家在市场上看到的健康信息改为：最近一次探测时间、最近 24 小时可用小时数、资源快照。出口 IP 对非卖家访问者隐去最后一段，卡片名称使用 `client-<installation 前 8 位>`，不公开卖家的物理子域名。

## 6. 空环境手动部署与验收

先部署含迁移 0128、窗口交接和逐批访问回执的 Router，再更新 Server 及其配套 Web 资源。过渡期内旧 Server 使用内嵌控制台。以下步骤适用于公网环境验收。

1. 在新环境为 Router 准备独立数据目录、空数据库、域名、TLS 和 owner 设置，按正常启动方式初始化。Router 的入站 SSH host key 用于 Client 反向隧道。
2. 在另一个空数据目录初始化 Server，配置同一 Router、Client 子域名和卖家身份，验证 owner 并建立反向隧道。Client 进程直接处理资源探测。
3. 从市场「我的上架」选择卖家自己的 Client，运行只读预检并发布。检查市场卡片上的出口 IP 与地区、CPU、内存、磁盘和最近探测时间。新环境的可用小时数从 0 开始，首个达到 10 个成功时段的完整 UTC 小时结束后应计入过去 24 小时的统计。
4. 用第二个账户报价并确认租用，检查独立租户工作区、卖家协助留痕和终端权限。若验证第三方车位，让第三个账户购买租户工作区内的 Share 车位。
5. 租户退租后核对旧票据立即失效。第三方车位须先撤权结算，卖家工作区恢复并完成本次清理回执与待处理结算后，卡片应自动恢复可租。最后由卖家下架并重新上架，记录公网链路的实际验收结果。

## 7. 信任关系

| 一方 | 能做什么 | 与 SSH 模式比 |
|---|---|---|
| 卖家 | 拥有机器 root，技术上能读租户数据；界面上只有脱敏的协助视图，操作留痕且租户可见 | 读数据的能力相同；新增留痕和告知 |
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
| 卖家想在租期内下架、删除或改 owner 邮箱 | 不允许 |
| 卖家在租期内改出站代理 | 锁定 |
| 卖家升级后出现问题 | 自升级流程已有健康检查与回滚；停机计入探测记录 |
| 卖家想绕过冻结使用租户账号 | 回环同样需要租户 token；root 直接读盘只能靠告知与信誉约束 |
| 卖家 Client 版本过旧 | 不具备 `marketLeaseV1` 时不能上架；已上架后若降级，暂停接新租约 |
| 租户 Share 上仍有第三方车位 | 先走 Share 立即退租与结算，再下发 `lease_end` |

## 9. 分期与验收

| 阶段 | 内容 | 验收要点 |
|---|---|---|
| P0 供给切换 | 建立在线 Client 上架入口 | 市场仅展示在线 Client 供给 |
| P1 Client 端 | 配置拆分与数据迁移；工作区与切换；三类身份；管理接口审计与标注；协助模式与访问记录；`/_ctl/market/*`；本地自治清理；出口探测；资源快照；`marketLeaseV1` | 卖家数据在租期内不可用、租后完整恢复；租户看不到 installation 级数据；回环拦截生效；指令幂等、过期序号被拒绝 |
| P2 Router 端 | 上架项与前置检查；租约流程；租户子域名映射；Share 按工作区归属与租后结束；Web 委托调整；主动探测与结算改造 | 租户建的 Share 收益归租户；租后旧入口立即失效；离线到期能补清理；核对前不能再租 |
| P3 前端 | Clients 页上架入口；市场列表改为上架项；租户控制台；卖家协助视图与访问记录；退租前备份；告知文案 | 遵循 CLAUDE.md 设计系统（44px 触控、h-12 输入、`ring-2 ring-accent ring-offset-2`） |
| P4 空环境切换 | 移除 SSH 托管入口、出站 provision 密钥、远端任务和独立 standby | 空数据库可初始化，在线工作区、主动探测、退租和反向隧道工作正常 |

市场页从在线上架项读取地区、资源、最近探测和可用小时。买家在同一张卡片获取报价并确认租用。卖家从账户页查看自己提供的 Client 及历史租约。[在线市场](../frontend/components/dashboard/client-market/online-market.tsx) · [卖家账户页](../frontend/components/dashboard/account-client-section.tsx)

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


## 11. 实现与运维核对

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
- 本地浏览器回归可用 `frontend/` 下的 `npm run test:client-checkout-browser`、`npm run test:client-termination-browser` 和 `npm run test:client-rental-audit-browser` 重跑。双进程浏览器用 `npm run test:client-workspace-browser`，从标准输入读取一次性夹具 JSON。字段为 `origin`、`installationId`、`sellerEmail`、`sellerToken`、`sellerPassword`、`buyerEmail`、`buyerToken`。夹具使用可发布的测试 Client、已登录的两个独立账户及开启的卖家终端。可选 `ownerLocalOrigin` 用于验证本机直连。设置 `WORKSPACE_LONG_SESSION=1` 可验证跨 10 分钟的工作区会话。[浏览器验收脚本](../frontend/scripts/client-workspace-browser-test.mjs)

| 本地回归 | 结果 |
|---|---|
| 数据兼容 | 在 0127 库中保留一份上架、活跃租约和旧访问回执，再执行 0128。原数据与外键检查通过，原回执次数为 1，待确认批次初始化为空。 |
| Router | Rust 1,986 项通过。前端 TypeScript 单测 579 项、国旗资源测试 5 项通过，类型检查和生产构建通过。 |
| Server | Rust 共 4,783 项通过、5 项按原标记忽略，含 139 项 API 契约测试。Web 单测 454 项通过，类型检查和生产构建通过。 |
| 契约与审计 | Server 审计测试 2,207 项通过。262 条 Web 命令均有工作区权限分类，Provider 覆盖、UI 矩阵及写入路径审计通过。 |
| 浏览器专项 | 结账、续费重报价、原页充值、过期及回执重放通过。退租争议和管理员审计通过中英双语、键盘及多宽度回归。 |
| Server 本地发布检查 | `RUN_TESTS=1 RUN_REAL=0 scripts/release-readiness.sh` 返回 `contract_verified`。本地启动、密码登录、Provider 与 Share 冒烟通过，失败 0 项、内部阻塞 0 项。 |

公网域名与 TLS、生产部署、真实上游 Provider/OAuth 调用、安装新版本与回滚二进制仍待环境验收。
