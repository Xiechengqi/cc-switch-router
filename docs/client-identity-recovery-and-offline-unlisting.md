# Client 身份恢复与离线下架

状态：已实现，待动态验收。2026-11-27。本文件覆盖普通离线清理、永久失联供给退出和历史孤立域名恢复；不得通过 IP 或邮箱猜测身份。

## 行为契约

- 六小时只回收运行记录；持久身份登记、公钥、已验证 Owner、域名预留和业务义务保留。原密钥重新注册返回原 installationId，控制凭据重建后持久化，旧会话不复活。
- Owner 下架立即关闭市场和新报价。已有租约、账务和清理独立推进，本机下架意图不得覆盖租约命令。
- 原机永久丢失仍可下架。离线同步进入等待重连，不产生虚假的网络请求次数。重连先协调遗留状态，再允许重新出租。
- 子域名接管、封禁保持不可逆身份 fencing；不因恢复流程重新启用旧身份。
- 旧 unlocking 迁移为已下架，保留控制任务。未确认清理不等于已清理，禁止重新出租。

## 历史修复

仅迁移仍存在的可信身份。旧身份已被清理且域名目录没有公钥时，需要管理员预检与显式应用：核对可信备份中的原公钥、域名修订、旧/新 ID、业务引用及撤销状态，事务恢复原 ID 并记录审计。不得上传私钥，不得覆盖 Provider/Account 数据，不得直接裸改主键。

## 实施及验证

依次完成身份登记与注册恢复、离线下架与权限保护、协调器、Server 依赖顺序、前端及旧设计清理。补齐恢复、错误密钥、回执丢失、并发下单/下架、在租下架和重连回归用例。

遵守用户限制：不部署、不重启远程服务、不执行运行类或 UI 测试。执行格式、编译、类型检查与纯源码审计；整体 review 记录未执行的动态验收项。生产身份修复必须单独预检，不在启动或迁移中猜测修复历史孤立域名。

## 实现索引

- `schema/0132_client_identity_and_offline_unlisting.sql`：持久身份、历史别名、下架意图、发送计数、租约/清理保护与旧 unlocking 迁移。
- `src/store/client_identities.rs`：身份读取、历史修复预检/应用、事务审计。预检默认不应用；生产修复没有执行。
- `src/online_client_market/lifecycle.rs`：Owner 事务下架、租约完成后的本机 unlist 调度。
- `src/online_client_market/reconcile.rs`：在租下架继续履约，离线不发送，已下架仍协调遗留任务，路由激活唤醒等待任务。
- `src/store.rs`：注册恢复原 ID/Owner/激活时间，GC 保留通知序号、域名与健康事实。
- Server `state.rs`、`state/background.rs`、`clients/router/tunnel.rs`：保留已知身份、显式别名恢复、控制隧道就绪后恢复 Share、冲突阻塞与无告警依赖等待。

Owner 在租和历史详情通过卖家专属 GET listing 状态读取独立的同步状态；已下架项不会因此回到当前挂售列表。

## 生产遗留项处理

`r0pu9iwq81` 使用历史恢复预检/应用工具核验可信旧备份，恢复原身份后由正常注册及 claim 收敛。violet 原实例已丢失：旧 unlocking 迁移为已下架，Owner 也可幂等下架；本机同步只等待重连，不要求找回服务器。没有生产部署或数据修改。

## 整体 review 检查项

- 原身份恢复不修改本机 Provider/Account 或根密钥；未知公钥、撤销身份和域名冲突拒绝自动接管。
- 下架与报价提交事务复核，已有租约继续，Owner 不获得租户权限，清理/财务义务继续阻止危险回收和重新出租。
- 等待重连不累计命令发送次数；旧回执不能重新挂牌；单个唤醒失败不能结束协调器。
- 原通知事件序号跨 GC 保留，避免恢复后去重键重用。健康数据使用独立 retention。
- 新增回归用例仅编译，没有执行运行类或 UI 测试。最终静态验证结果在任务完成报告中记录。

## 2026-11-27 首次静态验收与整体 review 记录

整体源码 review 已完成，当前范围内未发现未处理的阻塞项。复查修复了通知序号跨 GC、身份改变后旧 tunnel spec、已下架在租详情、归还命令与下架意图优先级、撤权独立于隧道恢复、网络校验失败退避，以及 marketListed 与租约保护混用的问题。Owner 专属状态接口在运行记录被回收后仍可查询同步意图；未确认本机同步不再被表述为市场下架未完成。

静态结果：Router / Server 的 `cargo check --all-targets` 通过；Router 全目标 Clippy 通过但仍有仓库既有 lint 警告，Server 全目标 Clippy `-D warnings` 通过；Router 前端 TypeScript 无输出通过；两仓全部 Rust 源文件格式检查及 diff 空白检查通过；Server Provider Phase 0 证据生成/检查与文档索引审计通过。新增后端回归用例随全目标编译通过，未执行。

未执行：服务启动、网络联测、迁移执行测试、运行类回归、UI/浏览器测试、部署、远程重启及生产身份修复。未将这些项目标记为通过。生产 violet 状态和 r0pu9iwq81 数据没有被本次工作修改，须在后续获准部署/数据处理后使用新行为与预检工具。

## 2026-11-27 历史恢复补充修复

随后只读核验发现：132 迁移执行前原运行身份已经丢失，迁移只能登记临时新身份；不能从仅含 ID 的域名目录反推出原公钥。同时恢复入口存在实现 bug：先插入原 ID、再删除临时 ID，会违反生产数据库 `idx_installations_public_key_unique`。旧预检只校验条件，没有覆盖实际写入顺序，因此预检成功不能证明应用可完成。

本次改为共享预检/应用计划，并在一个事务内先删除临时运行记录，再完整重建原运行记录。保留原始 SQL 类型及所有列，包括控制凭据、版本、升级授权和可选元数据；保持唯一索引和外键约束开启。审计写入失败同样回滚身份删除与重建。原域名目录不改主；Provider、Account、Share 本地存储和加密根密钥均不写入。

预检返回 `planHash` 与将退休的外键运行记录。应用携带 `expectedPlanHash`，复核域名修订、Owner、公钥、稳定运行属性、控制凭据和引用；引用不安全或计划改变即拒绝。业务恢复/清理表即使声明 `ON DELETE CASCADE` 也不自动允许删除。活跃心跳和地域采样不使计划持续失效。现存别名使同一恢复请求幂等。

恢复同时取原/临时身份的已知最大离线通知序号，重建未启用监控、在线状态未知的通知基线，避免序号退回零后复用历史去重键。该序号纳入预检哈希；不会把旧离线事件或残留心跳当作当前在线证据。

回归用例覆盖真实唯一索引、必需预检/过期预检、心跳不使预检失效而升级授权变更使其失效、阻塞外键/未知级联引用、可选列和控制凭据保留、离线通知序号延续、故障注入审计回滚、外键一致性以及重复应用。CI 明确选择这些测试并拒绝空过滤；本地仅编译，不执行。

## sgptokenswitch 的具体恢复记录与操作材料

本节是已核验证据与待执行步骤，**没有执行生产应用、部署或服务启动**。现场状态可能变化，应用前必须重新预检。

| 项目 | 已核验值 |
| --- | --- |
| 原 Client ID | `d5920f1b-c59f-4dc4-bc2c-f432037edc16` |
| 临时 Client ID | `f08cb9e8-b1e8-447c-9e0d-aff0f19da839` |
| 保留域名 / 修订 | `r0pu9iwq81` / `1` |
| 已验证 Owner | `router@sgptokenswitch.cc` |
| 当前公钥 SHA256 前缀 | `d7d77ff544e49234`；实际核验使用完整公钥 |
| 临时身份创建 | `2026-11-27 02:33:50 UTC` |
| 132 迁移应用 | `2026-11-27 06:28:39 UTC`，晚于原身份丢失 |
| 只读检查时安装版本 | Router `3708a27`；Server `f1e3e0dbb77f` |

两份旧备份均将原 ID、域名和已验证 Owner 绑定到当前完整公钥。Router 运行及持久身份当前只存在临时 ID，原域名仍属于原 ID。当时核实了 fencing、租约、挂售和 Share；该次记录遗漏了临时 ID 的终态升级任务与费率回执，详见下文。Server 在检查时未运行，不需要为准备请求而启动。发现 21 个旧 Share 域名墓碑，其中仅 14 个在旧备份中仍有效且与当前本地启用 Share 对应；后续只由本地有效 Share 正常恢复，不能批量复活历史删除项。

`scripts/ops/prepare-client-identity-recovery.py` 使用 Python 标准库：以只读连接核验 Router 数据库与备份 SHA256，读取 Server 配置中的公钥、ID、Owner 和域名，生成仅含公开恢复字段的请求。不读取 Server SQLite，不输出私钥、controlSecret 或供应商账号，不调用恢复 API。`--output` 仅创建新的 0600 请求文件，不覆盖已有文件。

以下命令作为现场材料保存，当前未执行。备份/数据库路径属于远程 sgptokenswitch；脚本应由后续允许的交付流程提供，修正后的 Router 恢复 API 可用前不得应用。

```bash
python3 scripts/ops/prepare-client-identity-recovery.py \
  --router-db /root/.cc-switch-router/cc-switch-router.db \
  --server-config /root/.cc-switch-server/server.json \
  --original-id d5920f1b-c59f-4dc4-bc2c-f432037edc16 \
  --replacement-id f08cb9e8-b1e8-447c-9e0d-aff0f19da839 \
  --subdomain r0pu9iwq81 --expected-host-revision 1 \
  --trusted-backup /root/.cc-switch-router/deployments/host-health-20261123-final/before.sqlite \
    990769b7149b497335d8b4579bf7ec7b44fb41c307aba789665a1a695b75bb7d \
  --trusted-backup /root/.cc-switch-router/deployments/host-health-20261123-pAvcs/before.sqlite \
    0e6d5bd22bbec34ce8ca7d6a0dfc5ce7d730d31bdd67bb6654d9cf6396edffaf \
  --output recovery-request.json
```

1. 应用前用受支持的 SQLite 在线备份方式备份当前 Router DB，保持 Server 现有配置、数据和根密钥；不能用旧 Router 备份覆盖整个生产库。较旧的远程 Python SQLite 3.37.2 不能解析 Server 数据库中的新 JSON 运算符，这不是数据损坏证据，不能因此清空 Server 数据。
2. 使用已有 Router 管理员会话提交 `recovery-request.json`，调用 `POST /v1/admin/client-identities/recover`，保持 `apply=false`。核对原/临时 ID、域名修订、Owner、`planHash` 和引用列表。准备工具不替代 API 的 fencing/业务义务/外键检查。
3. 预检无冲突后，由同一请求追加 `apply=true` 与 `expectedPlanHash=<预检返回值>` 调用同一接口；如果状态变化，重新预检，禁止跳过约束或直接改主键。成功应只有一个原身份运行记录、一个原身份锚点、一条临时 ID → 原 ID 别名和一条恢复审计；域名仍属于原 ID。
4. 后续允许 Server 运行或显式重连时，注册返回原 ID 及含临时 ID 的 `previousInstallationIds`；Server 原子保存身份并依次恢复 Client 控制隧道、有效 Share 描述和 Share 隧道，最后发送健康刷新。原子域名和供应商账号数据继续使用，不执行 init，不清空目录。
5. 后续动态验收确认 Router 心跳在线、子域名可达、14 个有效 Share 的实际状态，以及本地账号数量与身份未变。未完成这些检查前，不报告生产恢复成功。

Server 同步修复：本地初始化标记不随 Router claim 失败回退；本地登录保持可用且匿名初始化入口关闭，在租 Owner 仍被禁止管理。诊断区分注册、连接、域名冲突和身份恢复，并保留无 URL 时的具体错误。模型健康刷新先排队，依赖有效 Client/Share actor 和描述 ACK，失败或并发更新保留队列，进程重启按有效本地 Share 重建待刷新项。

## 本轮整体 review 与静态验证记录

已复核恢复事务的唯一键/外键顺序、完整运行列和控制凭据保留、预检失效条件、fencing/接管/业务引用保护、通知序号和审计失败回滚，以及 Server 初始化、在租隔离、当前完整 tunnel spec 和健康通知排队边界。本次范围内未留下已知阻塞问题；生产应用和动态正确性仍待后续验收。

- Router、Server 的 `cargo check --locked --all-targets` 通过，回归用例仅编译。最终 Router 全目标 Clippy 通过（仓库既有警告仍存在）；Server 全目标 Clippy `-D warnings` 通过。
- Server Web TypeScript、Provider Phase 0 生成/检查、Provider coverage、UI Provider matrix、Web runtime/产品边界/i18n 及文档索引审计通过；Router Share 模型健康契约审计通过。
- 本次修改文件的 Rust 格式及两仓 diff 空白检查通过。Server 全部 Rust 格式检查通过；Router 全仓格式检查在本次范围外的 `src/online_client_seller_summary.rs`、`src/schema.rs`、`src/share_market_audit/tests.rs` 报告既有格式差异，未改动这些文件。
- 恢复准备脚本仅作 AST 语法检查；新增 CI 过滤器通过源码核对，均匹配实际测试声明。未运行准备工具、运行类测试、UI 测试、服务、部署或生产恢复 API。

## GitHub Actions 后续修复

提交 `8dc9929` 的 [Actions 45972127168](https://github.com/Xiechengqi/cc-switch-router/actions/runs/45972127168) 中，前端、发布包编译和普通身份恢复用例通过，历史身份恢复用例在首次预检报 `no such table: provisioning_jobs`，发布因此被阻止。原因是本轮预检误加入了 `provisioning_jobs`、`subdomain_reservations` 两张已由迁移 127 物理删除的 SSH 供给旧表，前次静态 review 漏掉了此处迁移边界；这也会阻止真实恢复接口，不是 CI 环境缺表。

删除这两项旧依赖，继续校验现行 `online_client_*`、租约、财务/清理、Share、命名空间、fencing 和外键义务。不能创建空旧表绕过、修改已发布迁移，或把数据库异常当作无业务义务。现有历史恢复用例明确断言旧表不存在，然后覆盖预检、应用、回滚和幂等全链路；本地仅编译与源码检查，执行结果以自动 CI 为准。

## sgptokenswitch 历史身份恢复第二次核查（2026-11-27）

原身份最后一次心跳为 11 月 26 日 15:20:02 UTC，21:22:51 Router 记录 `installation_removed`。临时 ID 于 11 月 27 日 02:33:50 创建，身份持久化迁移 132 于 06:28:39 执行，因而公钥只被登记到临时 ID。10:40 的启动已注册该 ID，随后认领保留的原域名返回 409。10:54 只读核实了临时 ID 下的失败升级（无待重启、租约字段为空、无租户任务前缀）及一个当前全局资源费率回执。两者都是同次注册后已有的记录。

预检按实际记录列出可保留的历史、需要重新签名的回执和阻塞原因。升级历史在原 ID 下保留全部列与最新上报时间。状态、目标、日志、错误详情或发起人变化会改变哈希，预检和审计只保存任务摘要。全局费率快照继续保留，Server 用原 ID 确认当前适用版本。历史版本仅在仍有有效授权引用且本地已安装时，才按既有协议重新签名。专属发布历史、额度投影、未结租约、待重启升级、租户任务及未知外键引用（包括升级历史的子记录）都会阻止应用。重发已提交的请求可取得原审计回执。

现场操作顺序（当前仍待生产执行）：

1. 记录 Router/Server 的运行版本，使用支持 WAL 的一致性备份方式保存当前 Router 库。通过 Server 已有受控读取方式记录 Provider 与账号 ID 集合摘要、有效 Share 的 ID 与子域名；保留本地数据库、配置及加密密钥。旧 Router 备份仅用于证明原身份。
2. 用本页准备命令重新生成请求。旧备份使用其 SHA256 覆盖的独立 SQLite 文件，当前 Router 库按只读事务读取已提交的 WAL。核对当前公钥、已验证 Owner、域名修订以及原 ID 对应的路由和目标 lane。对修正版 Router 发送 `apply=false`。上次核查对应一条应保留的失败升级和一个应退休的全局资源费率 ACK。以此次预检返回的清单为准，出现 `details.blockers` 时逐项处理实际义务。
3. 使用刚取得的哈希显式应用。响应丢失可重发原请求。核对原 ID 唯一、临时 ID 到原 ID 的别名、域名归属、升级历史及唯一审计回执。
4. 已运行的 Server 可通过心跳重新注册，或使用“重新检查”。核对原 ID 的新心跳、原域名控制隧道、当前费率的签名 ACK 及到期后的激活。按本机启用清单逐项核对 Share 的域名、描述确认和传输，并比较恢复前后的 Provider 与账号 ID 集合。上次清单为 14 个有效 Share、21 个历史域名墓碑。部分 Share 延迟时按该 Share 的同步状态重试，毋需回退已完成的身份事务。

本轮整体 review 检查了预检与应用之间的状态变化、审计写入失败时的事务回滚、重复应用、升级任务子记录的级联删除风险，以及 Server 接受原 ID 后的控制隧道与 Share 恢复顺序。回归用例覆盖实际六小时清理入口、资源费率重新签名与激活、控制隧道任务已退出，以及一个 Share 的描述被拒绝时其他已连接 Share 仍可独立确认并上报健康。此次验证限于编译和静态检查，现场恢复结果待应用后验收。
