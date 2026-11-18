# Share 扩展能力授权与计量覆盖：设计方案及完整实施计划

> 状态：代码实施、整体 review 和本地回归已完成；未部署。真实 Provider / App / 模型和生产采用保持 `live_pending`。实际交付与验证见第 14 节，前面的验收要求不等于真实上游已经通过。
>
> 日期：2026-11-17。适用仓库：`cc-switch-router` 与 `cc-switch-server`。
>
> 核对基线：Router `dbbd308`，Server `92fce0f`。Router 起始已有 16 个修改中的文件，原始 diff 已单独保存；实施保留其批量水位、价格与布局改动，并衔接有关 `unpricedUsage` 合同。Server 起始工作区干净。
>
> 现行线协议以 [PROTOCOL.md §13.6](../PROTOCOL.md) 为准，Server 实现规范见相邻仓库 `docs/share/extended-capabilities.md`。本轮不含提交、推送、部署和真实付费请求。

## 1. 确定的产品设计

1. 在 Share 设置增加独立的「扩展能力」配置。Token、美元、未设置个人额度三种情况均可查看和管理，不再依附美元额度复选框。
2. 控制会执行的操作：图片生成、图片编辑、视频生成，以及当前 Driver 实际实现的音频输出、供应商执行工具等。新 Share、新增能力和新加入的运行时默认不获得额外许可。
3. 图片、PDF/文档、音频、视频作为模型理解输入，继续按原模型输入能力使用。普通客户端 function/custom tools、结构化输出、推理、缓存继续属于模型交互；不因名称或数据类型被当作付费服务端工具。
4. Share owner 管理能力上限。普通用户 grant 可继承或收窄；市场 grant 使用冻结的明确集合。调用 owner 自己的 Share 也检查该 Share 的能力策略。
5. 每项能力同时展示「能否执行」「如何计量」「美元如何定价」「是否扣当前额度」。估算、部分覆盖、缺价、无法采集和未知必须显式区分。
6. 能力开启不承诺已经完成计费。默认要求当前有限额度覆盖该能力；普通 Share 可由 owner 对具体作用域明确允许未覆盖消耗，市场额度不允许此例外。
7. 无法计量不显示为真实零消耗，缺价不显示为免费。已知维度照常记账，一个资源缺价不能把整个请求的已知费用抹成零。
8. Server 是操作识别、能力事实、权限执行和权威用量的来源；Router 负责编辑、诊断投影、价格发布、市场合同和用户展示。两端使用相同语义和契约夹具。
9. 保留当前周期消费、人工调整、在途风险、历史价目和异步任务归属。修改能力不重置用量，切换单位不自动改变能力选择。
10. 从配置到真实转发逐层生效：设置预览、保存重验、每次生成准入、结果采集、结算与恢复，不能仅增加前端复选框。

本轮不新增尚不存在的供应商接口，不把媒体张数或秒数伪造成 Token，不改钱包余额或市场租金算法，不引入新的「按张/按秒额度」产品。未来可增加原生资源额度，但本轮必须如实说明现有 Token/USD 额度能覆盖什么。

美元始终指冻结执行价目计算的 API 等值额度，不能解释为 OAuth 订阅真实支出或供应商账单。

## 2. 实施前基础与衔接点

本节记录核对基线的差异；实施后的结构、实际版本与限制见第 14 节。

| 现状 | 证据位置 | 设计处理 |
| --- | --- | --- |
| Router 的资源开关只在美元模式显示，并把布尔值映射成 v1/v2 价卡 | `frontend/components/dashboard/share-edit/share-user-grants-editor.tsx`、`value-quota-fields.tsx` | 用独立能力编辑器替代用户可见的资源总开关；价卡版本保留为内部协议 |
| Grok 已有默认关闭的图片生成、编辑、视频生成许可 | Router `src/models.rs`；Server `src/domain/sharing/router_contract.rs::GrokMediaPolicy` | 迁移已有明确许可，最终收敛为供应商无关的能力策略 |
| Server 已有账号能力证据的支持/不支持/未知、来源、过期时间、凭据身份版本 | Server `src/domain/accounts/capability_evidence.rs` | 复用事实模型，扩大操作维度；未知不能直接变成支持 |
| Provider Registry 是 Profile/Driver 与运行时权威 | Server `src/domain/providers/registry.rs` | 在 Registry/Driver 合同中声明已实现操作及采集、拦截责任，不用 Provider 名称白名单替代 |
| `is_metered_text_value` 将图片、文件、音频等输入归入非文本，`requires_v2` 又依赖此分类 | Server `src/domain/usage/value_pricing.rs`、`value_pricing/resource_request.rs` | 拆成操作权限识别、输入模态事实、计费维度计划，禁止复用旧分类器作为新能力开关 |
| `resource_metering` 就绪主要证明协商和价目激活 | Server `src/state/share_value_quota.rs` | 新增按绑定/模型/操作/单位计算的诊断；总布尔值不能证明所有资源完整计费 |
| 普通 USD 缺价准入可产生整个 attempt 的零价标记；资源策略检查位于模型解析成功分支 | Server `src/repository/server_sqlite/value_quota.rs::admit/settle` 相应逻辑 | 权限先于价格检查；按维度记录覆盖和金额，封住缺模型时绕过操作许可的路径 |
| 已有每次生成独立 attempt、整数 picoUSD、冻结价、在途预占和异步任务 | Server `src/state/share_value_quota*`、`src/repository/server_sqlite/value_quota*` | 扩展现有事实和结算，避免第二套资源账本 |
| 已有带版本的 Share 设置、operation ID、指纹、回执和 v12 单位转换 | Router `src/ctl_client.rs`、`src/store.rs`；Server `src/state.rs`、`share_value_quota/conversion.rs` | 能力与用户额度同一配置事务保存，复用冲突和重放保护 |
| Router 成本明细仍主要基于 Token 分桶和重新估价 | Router `src/store.rs`、`src/models.rs`、`frontend/components/dashboard/drawer-panels.tsx` | 权威扣减明细读取冻结账本；当前价估算另列，不混成同一个金额 |
| Router 工作区新增 `unpricedUsage: zero/reject`，当前 Server `ValueQuota` 尚不接收该字段 | Router `src/share_value_pricing.rs`、`frontend/lib/types.ts`；Server `src/domain/usage/value_pricing.rs::ValueQuota` | WP0 审核后衔接，不能假设已端到端可用；新字段必须完成协商和 Server 执行 |

当前 Server 核心 SQLite schema 已是 **7**，不是早期资源文档中的 6；Router 迁移已到 **0116**。下一版候选为 Server schema 8、Router migration 0117，实施前重新检查占用，只追加新迁移，不重写已发布迁移。Share 合同当前最高使用 v12，新功能拟使用 v13。

Router 当前修改还包括批量水位上报、价格展示和账号信息布局。实施前逐项记录归属与依赖，仅合入本功能需要的部分，不把全部工作区变化视为本计划成果。

## 3. 能力目录和操作识别

### 3.1 操作标识

使用稳定的供应商无关 ID；ID 表示操作，不表示 HTTP 路径、输入格式或价格单位。

| 类别 | 首批 ID | 开放条件 |
| --- | --- | --- |
| 图片生成 | `image.generate` | 当前实际执行路径已实现且能在发包前执行权限检查 |
| 图片编辑 | `image.edit` | 同上；编辑素材属于该操作输入，不另算输入许可 |
| 视频生成 | `video.generate` | 当前 Grok 媒体执行路径；不能把目录存在解释为其他 Driver 已实现 |
| 音频输出 | `audio.generate` | 只为已实现的模型音频输出路径提供选择；没有独立 TTS 路由时不得宣称存在 |
| 供应商执行的工具 | `tool.web_search`、`tool.web_fetch`、`tool.file_search`、`tool.code_execution` | 根据实际 Driver 的工具执行合同识别；不是所有同名工具都在供应商执行 |
| 其他供应商执行工具 | `tool.computer`、`tool.remote_mcp` | 仅在确有服务端执行及可靠权限控制的路径启用；客户端执行的 computer/bash/MCP 不归入此处 |
| 后续能力 | 视频编辑/扩展、独立转录、embedding、rerank、batch 等 | 目录可说明未实现，但不可勾选；新增实现必须同时增加权限、用量与测试合同 |

Server 当前明确存在的独立媒体入口见 `src/api/mod.rs` 和 `src/proxy/forwarder.rs::forward_*images/forward_grok_media`；当前 `InferenceOperation` 尚无通用独立 audio 入口。首批可选集合由这些实现反推，不以本表预期代替源码核实。

文本生成是基础操作，内部使用 `text.generate` 参与用量诊断，界面无需额外默认勾选。模型读取、countTokens、已创建任务状态读取等为辅助操作，不混进「生成」许可。

### 3.2 目录、当前支持与运行时事实

每一条能力事实绑定：installation、Share、App、Provider/Profile/Driver、最终模型或已知模型范围、执行路径、账号身份 generation、配置 revision、采集器版本。

目录与实例事实分开：

- 目录声明当前 Server 二进制实现什么操作、识别哪些请求语义、收集哪些用量维度。
- 实例事实声明当前绑定/账号是否启用、模型是否支持、证据是否过期、是否存在可执行路径。
- 上游账号可用性未知时显示「待验证」，而非「支持」或「不支持」。Driver 已实现且能够可靠拦截时，普通 owner 可以明确启用待验证账号；这不把其事实改成已验证。
- 没有实现或无法可靠执行权限控制的路径不可通过 owner 勾选绕过。价格存在不能提高执行支持状态。
- 显示部分模型支持时，展开列出受支持范围与失败原因；未知/开放模型集合在实际请求时重验，不构造虚假的覆盖百分比。
- 查目录和预览不触发付费请求或自动上游探测。真实验证沿用显式测试入口，并记录来源与时效。

### 3.3 分类必须覆盖的入口

同一语义操作，无论经专用端点、Responses 内置工具、Gemini 输出模态、兼容 Chat、WebSocket、Driver 内部桥接或别名，都落到相同 ID。

分类读取协议规定的控制位置与实际映射后的模型，不递归扫描任意用户数据寻找关键词：prompt、function schema、普通工具参数、历史生成结果均不能误判为新生成。用户函数名叫 `image_generation` 也不等于供应商内置生图工具。

应识别的是本次上游请求可能执行的供应商能力。真实 hosted tool 声明即需要许可，即便本次最终没有调用；计费只在观察到实际用量或依据明确估算规则时发生。仅被 Driver 剥离的被动 namespace 声明不得把普通文本请求错误升级成生图请求。

WS continuation、`previous_response_id` 等可能恢复工具或模态状态；新一轮生成必须合并该会话已保存的执行能力集合后重验，不能因为本轮 body 省略 tools 就绕过关闭后的策略。

不能通过“结果出来再删除图片”实现禁止生成。对于可能隐式产生媒体的路径，必须证明请求参数、模型模式或 Driver 设置能限制输出；无法证明时标记控制不完整，不能宣称能力开关可靠。

## 4. 授权、作用域和生效规则

### 4.1 配置结构

新契约采用以下语义，最终 Rust/TypeScript 类型在 WP1 固化：

| 字段 | 语义 |
| --- | --- |
| `Share.capabilityPolicy.version` | 独立策略格式版本，首版为 1 |
| `Share.capabilityPolicy.allowedOperations` | Share 允许的扩展操作显式集合；空数组表示全部关闭 |
| `Share.capabilityPolicy.operationBindings` | 各操作已确认的 App/运行时绑定身份集合；由 Server 根据预览生成并验证，不能用通配符给未来绑定授予许可 |
| `ShareUserPolicy.allowedOperations` | 普通 grant 缺省表示继承 Share；显式空数组表示不开放扩展操作；显式列表只能是 Share 集合的子集 |
| Server 保存的 coverage acknowledgements | owner 对特定作用域、操作及未覆盖条件的明确确认，不由浏览器自报已验证事实 |
| 市场合同中的 capability snapshot | 明确操作集合与计量要求，随 seat/quote/subscription/trial/grant 冻结 |

`capabilityPolicy` 在旧 Share 中缺失表示旧协议兼容态；在新 v13 Share 中必须存在。未知字段、重复操作、未知 ID、非法枚举在写边界拒绝，不能依赖当前部分 `ShareUserPolicy` 宽松反序列化而静默丢弃权限字段。

序列化必须保留显式空数组，不能使用会把空集合省略为继承态的规则。新建、编辑、批量、Provider Bundle 保存、导入/导出、CLI 与同步 reader 共同遵守同一校验；旧签名 payload 省略新增字段时保持原指纹。

绑定许可区分“当前这个 App 的这份运行时绑定”与泛化的 Provider 名称。新增 App、替换账号或改变 Driver 身份后，旧 `operationBindings` 不匹配，必须在保存预览中明确确认新绑定；仅刷新 OAuth access token 不视为替换身份。多个现有绑定可在一次 owner 操作中批量授权，不逐个弹窗。操作名称的开启意图可保留，但没有匹配绑定许可时不执行。

有效许可是：**当前实现能控制并执行 ∩ Provider 配置许可 ∩ Share 许可 ∩ 用户/市场许可 ∩ 当前请求的计量准入条件**。

普通 grant 的继承意味着 owner 以后扩大 Share 集合会影响这些 grant；界面保存前列出影响人数。需要固定权限的 grant 可设置明确子集；市场永不使用无界继承。

### 4.2 作用域

- 普通授权：按现有规范化用户身份与 grant 生效，个人额度优先，Share 总 Token/并发上限继续执行。
- 公开免费：仍要求有效的 Router 用户 API Token，匿名不得调用。没有个人 grant 的用户使用 Share 默认能力和明确的无额度/Share 总额度说明。
- 多额度并存：分别展示个人 USD 与 Share 总 Token 等各自覆盖；不把 Token 总上限描述为媒体成本总上限，也不因它无法表达秒数就否定完整的个人 USD 计费。
- 没有任何数量/金额限额：显示「未设置用量上限」，并继续显示计量、定价缺口；不伪称受某个预算保护。未计量风险的确认仍按当前操作事实记录。
- 本地 Provider 管理员测试与真实 Share 调用区分。外部调用不能伪造测试 scope 取得绕过；通过 Share 发起的连接测试使用相同 capability gate。
- Router owner 的管理权限不自动获得额外 Share 推理许可；保留既有查看/管理边界，不扩大原始账号或凭据披露。

### 4.3 禁用、生效与并发

只拒绝新的生成及新的实际重试/内部生成。已发送请求按原策略、价目和窗口完成；新 WS turn 即使在旧连接上也使用最新策略。配置提交与生成准入使用现有短 mutation gate，定义“提交前已准入”和“提交后新准入”的明确边界，不在网络等待时持锁。

已有异步任务的后台采集、对账和结算继续进行；查询/下载/取消仍检查身份和任务归属，不因生成开关关闭而自动失效。撤销身份或租约终止仍遵循既有权限规则。编辑、扩展、remix、重新生成属于新任务，重新准入。

绑定切换后重新计算支持和覆盖，不能把旧账号的未知风险确认扩散到新账号。保存中的开启意图可以保留为不可用状态供修复，实际调用必须拒绝不满足条件的路径。

## 5. 计量、定价与额度覆盖

### 5.1 状态模型

| 维度 | 状态与必要信息 |
| --- | --- |
| 执行与控制 | `supported / unsupported / unknown`，并给出控制是否完整、哪些模型/路径可用、事实来源及有效期 |
| 用量采集 | `observed / estimated / partial / unavailable / not_applicable`，列出原生单位、包含关系、采集器和证据来源 |
| 美元定价 | `priced / explicit_free / partial / missing / not_applicable`，价格来源、冻结 revision、参数范围、缺价维度 |
| 当前额度覆盖 | `full / partial / none / unknown / unlimited`，明确扣减哪些维度、遗漏哪些维度 |
| 准入保护 | `bounded_reservation / estimated_reservation / post_settlement / unavailable`，与结算覆盖分开，避免将事后计费宣传成硬上限 |

`observed` 表示取得当前协议声明的有效用量，不等于独立证明供应商真实物理消耗。静态夹具通过与真实账号 `live_verified` 另列，不能互相替代。

状态按具体绑定、模型、操作、请求参数、目标额度单位与有效价目计算。图片尺寸/质量、视频分辨率/时长、模型 tier 等不同条件可能得出不同结果。单个缺价模型不能阻塞该 Share 所有无关文本模型。

### 5.2 Token 与美元决策矩阵

| 实际情况 | Token 限额 | 美元限额 |
| --- | --- | --- |
| 模型理解图片/文件等输入，上游有完整 aggregate Token | 使用现有 Token 口径；细分是否独立计价另行说明 | 有完整适用价格时计费；缺细分或价格时标明具体缺口，不以输入类型一刀切 |
| 生图/音频返回可信 Token，采集已接入原有 Token 汇总 | 对实际纳入的 Token 扣减；不能把“存在字段”当作“已纳入额度” | 按正确 Token 维度、包含关系和价格计费 |
| 生图只有实际张数，没有 Token 语义 | 图片资源不计入 Token；可能只有交互 Token 计入 | 有该模型/尺寸价格且计数可靠时可完整计费 |
| 视频只有时长 | 视频消耗不计入 Token | 实际时长或明确估算计价，分别标注 |
| 搜索有次数、对话有 Token | 对话 Token 计入，搜索资源次数不计入 | 对话费用与搜索费用分别计算；避免覆盖或重复 |
| 有价格但数量缺失 | 不能伪造 Token | 无可靠估算时无法计费；有合法估算时明确标为估算 |
| 有用量但价格缺失 | 真实 Token 可继续计数，美元缺价本身不阻断 Token 额度 | 缺价维度未计费，不能叫免费 |
| 明确配置零价 | 真实 Token 仍按原口径扣减 | 金额零但仍记录实际用量、许可和并发约束 |

新采集器若增加实际能统计的 Token 维度，应升级 measurement schema，并注明从何时开始生效。历史记录不按新口径重新累加；同一次事实修订不能把 aggregate 与细分加两次。

### 5.3 未覆盖消耗的规则

默认 `require_coverage`。完全支持当前额度的操作可启用；明确的估算计量在现有软上限规则下可用，界面持续显示估算及预占性质。无法计入、只有部分计入或结果未知时，不因勾选能力就默许无额度保护。

普通 owner 可以在预览中逐项确认「允许此部分消耗不受当前额度限制」，确认范围绑定：Share、用户 grant 或公开默认策略、操作、App/运行时/模型范围、额度单位、遗漏维度及理由、控制/采集规则版本。确认由 Server 验证后随策略原子持久化并审计。

限制：

- 只有 Share/授权的既有管理者可确认，普通被分享用户不能自行解除限制。
- 例外只允许匹配的未知/未覆盖部分，已知 Token、金额、并发、到期和总限额继续执行；额度已经耗尽时仍按原准入规则阻断。
- 对不存在的 Driver 实现、无法控制的操作、持久化损坏和身份无效，确认不能绕过。
- 现有 `unpricedUsage=zero` 只表达 USD 缺价策略，不是扩展能力授权，也不表示缺失数量可以当零。新扩展能力的缺价例外必须另有明确确认；`reject` 继续优先拒绝缺价。
- 如果 owner 为开放缺价扩展操作同时把 USD 缺价策略从 reject 改成 zero，预览必须显示它对同一 grant 的文本缺价请求也有影响，不能隐式修改该设置。
- 新建 v13 USD 授权默认使用拒绝缺价；旧授权省略值的既有 zero 语义在兼容态保持。新规则不追溯变更历史结算。
- 单位切换、运行时/模型范围变动或新增遗漏维度使确认失效；仅价格数字正常更新且覆盖集合不变，不要求重复确认，但保持原价目生效延迟和每次请求冻结。
- 未设置额度时使用明确的 unlimited 状态，不伪造 coverage exception 为有限额度。
- 市场 grant/trial 禁止未覆盖例外。支持既有估算策略，但须在合同展示并冻结其版本与风险规则；不可宣称是精确计量或绝对硬预算。

owner 的显式确认是产品配置的一部分，不是开发实施过程中反复请求人工批准。

## 6. Server 的执行与账本设计

### 6.1 每次真实生成的流程

1. 校验身份、Share/grant、App、到期和原有访问限制。
2. 完成最终 Provider/Driver/model/tier 路由与协议转换，生成规范化执行意图；无副作用的凭据/会话准备不算生成。
3. 计算可能执行的扩展操作，读取当前 capability policy 与会话继承状态；拒绝未授权或无法控制的操作。
4. 生成按维度的用量计划，解析适用价格与当前额度覆盖，验证 scope 对应的明确确认。
5. 使用现有 mutation gate 与权威事务重验最新策略，记录 request/attempt、能力版本、资源计划、窗口、价目、预占及风险；提交后释放锁。
6. 对最终线上请求再次保证控制参数未被后续插件/Driver 注入扩大，然后发送；需要改变能力意图时返回前一步重新准入。
7. 在响应转换前采集原始用量事实；统一归一化、去重、标记精度和来源。
8. 正常结束、取消、超时、未读取 body、重试、异常返回均走同一 finalize；未知结果保留风险。异步生成在原 attempt 结算，轮询不新建生成收费。

每次新生成的重试、HTTP fallback、内部摘要、Cursor 内部再生成都重新执行步骤 2—8。认证刷新、任务轮询、重附着已有响应不重复记生成，但仍检查各自身份和归属。

### 6.2 价格与数量分离

资源计划应保存可能维度、请求数量、服务器实际施加的约束、估算数量与版本；实际结果保存观察数量。二者不可混成一个无法解释的数字。

现有工具 8 次、音频 128000 Token、视频默认 15 秒等只是预占/保守估算参数。没有真实上游约束时必须标为 estimated reservation，不能称作执行上限；实际消耗超过估算时照实记录、按现有风险规则结算并阻断后续超限调用，不能截小成计划值。

所有用户提供的张数、时长、分辨率、质量、batch 数量和 max-token 字段在最终协议边界验证；multipart、透传字段和内部桥接不能绕过相同约束。使用检查溢出的整数乘加，不以截断或负数金额吞掉异常；异常应保留原始有界证据并返回明确错误。

维护各原生 Token 维度与 aggregate 的包含关系，价格拆分不改变真实 Token 总数。以总上下文判定长上下文档位；只有已证明包含在 aggregate 的精确子量才扣除一次。张数/秒数/工具次数不与 Token 相加，也不同时按两个互斥定价口径收费。

### 6.3 分维度结算与异常

扩展现有 attempt metadata / frozen tariff fragment / observation 数据，保存每个维度的 quantity、unit、source、precision、pricing state、settled amount 和 coverage reason。

- 缺少一种资源价：对已知维度正常计费，缺价维度另记 unpriced，不生成整笔零价模型替代已知价。
- 所有维度均缺价：可以是已确认的金额 0，但 API/UI 同时返回未计价量及状态，不把该金额解释为免费或完整总价。
- 有价无量：采用该操作明确允许的估算/保守策略；无可靠依据时进入待对账状态，不能用 0 表示成功结算。
- 发送后出现计划外资源：保存事实。严格范围保留预占/风险并隔离相关 scope；普通例外仅覆盖明确允许的同类缺口，新增未知维度不得自动继承例外。
- 发送后发现未授权能力确实执行：记录执行违规、停止该受影响操作的后续新生成并告警；保留账本。已发生上游消耗无法靠删除响应撤销，必须修复控制路径后恢复。
- 失败、被取消或没有成功输出都不能单独证明没有上游消耗。只有未发送或已有明确免费证据才记免费终态。
- 定价修复、迟到观察或对账修订走现有持久 revision 路径，保存原始决策和修订审计。既有旧价/旧缺价行为按其冻结规则处理，不能批量重写历史语义。

### 6.4 Token 模式与转换

能力检查对 Token 请求同样执行，不依赖 USD ledger 的价格解析分支。Token 模式记录实际可获得的资源事实，并分别报告美元估值是否可用，供将来 v12 转换和诊断复用。

USD → Token 的历史资源金额没有自然 Token 对应时，继续使用已有转换预览、校准和在途风险规则；不做固定汇率换算。Token → USD 缺少历史资源事实时不能当作零。仅改变能力列表不建立新窗口、不刷新 metering origin、不重置人工调整。

v1/v2 保留内部价格表示。目标请求有独立资源定价维度时需要适用的 v2 价目及 active ACK；单纯多模态输入且只按现有完整 aggregate Token 定价时，不仅因输入类型就强制 v2。预览给出所需内部策略，保存时与用户确认的能力/额度变更原子应用。修改价卡策略不能改写旧 attempt 的冻结价。

## 7. 跨端协议、保存与持久化

### 7.1 协商与候选版本

新增独立能力 `shareCapabilityPolicyV1`，不复用 `resourceValueQuotaV2`。启用新控制语义要求 Router reader ≥13、Server 对应 writer/执行能力与新持久化 schema 就绪；资源价格协商、单位转换协商继续各自检查。

v13 表示 Server 能完整读取、保存并执行新授权，不只是能序列化字段。历史普通 descriptor 按现有版本发送；带新 capability policy/确认/市场快照的 descriptor 必须使用新版本。旧 reader 不得通过去掉字段接收同一 Share。

新策略的持久数据使用 Server 下一版 SQLite schema；正式启用前要求既有 authority 激活流程完成，不在预览中偷偷激活/迁移。该前提是新合同的可靠持久化要求，与选择 Token 或 USD 无关；Token 操作不要求美元价目已就绪。旧存储/旧合同继续明确标为兼容态，不假装新限制已生效。

### 7.2 查询与预览

已新增只读 `POST /v1/shares/:share_id/capability-preview`，经已有 HMAC 控制通道转发 `/_ctl/preview_share_capabilities`。Server Web 使用对应 invoke，独立 Share 页走 `/share-api/share/capability-preview`。

实施选择：新建 Provider Bundle/Share 先以扩展能力全部关闭创建，再对已持久化的真实绑定预览启用；不提供创建前虚拟绑定预览。这让预览、保存和发网门禁使用同一个真实运行时指纹，避免草稿绑定与实际绑定不一致。新建流程明确显示此步骤，不在后台提前授予权限。

请求包括 `expectedConfigRevision`、目标 Share capability policy、变化的用户 policies、目标单位及需要预览的操作/模型范围；沿用有界批量输入，限制诊断条数和模型枚举量。

响应包括：

- `configRevision / catalogVersion / runtimeFingerprint / pricingRevision / generatedAt / expiresAt`；
- 按绑定与操作的支持事实、measurement/price/coverage/enforcement 状态、原因和受影响 grant；
- 需要确认的未覆盖维度，以及由 Server 生成的短期 `previewCredential`；
- 是否需要内部价卡切换、v12 单位转换预览或旧策略采用步骤。

previewCredential 沿用现有 HMAC 绑定思路，最长 15 分钟且不超过有关证据/窗口有效期。绑定目标策略摘要、scope、配置/grant revision、运行时、缺口集合及相关价目；不能由浏览器提交 `ready:true` 替代。预览无上游调用、账本补写或历史补价副作用。

仅管理者可读取完整诊断；普通用户可从已授权 Share 的只读能力摘要查看本人有效操作与计量状态，不暴露账号 ID、凭据、其他用户额度和私有上游地址。响应 private/no-store；界面缓存按登录身份与 Share/绑定 revision 隔离，登出和切号清理。

Dashboard 与独立 Share 页的 `/share-api` 入口复用相同的管理/本人读取边界，不维护第二套能力事实。路由读写继续使用当前会话、Origin/CSRF 和管理角色检查；不能让查看诊断获得编辑 grant 的权限。

配置/绑定/账号身份/模型规则/价目变化会失效相应诊断缓存。失效范围限于有关 Share/模型；Server 发包前仍直接读取权威状态，不能信任前端或 Router 缓存的 ready。过期预览可显示为陈旧信息，但不能用于扩大许可或保存新的风险确认。

### 7.3 保存

复用 `apply_share_settings`、`editContext.operationId/expectedConfigRevision`、每用户 grant revision 和 settings receipt，增加独立 capability patch 及 coverage acknowledgement 输入。

Server 在同一事务内校验并写入 Share/grant 策略、scope 确认、必要的价卡/单位转换和持久回执。批量编辑原子成功或失败，不能部分用户启用、部分用户仍旧。同 ID 同 payload 重放只返回回执；同 ID 不同 payload 拒绝。

Router 使用返回回执与 descriptor 校验完整应用；同步失败回落现有待处理队列。队列等待期间凭据可能过期：执行时重验并返回可恢复冲突，不能自动接受变化后的风险，也不能因浏览器预览时 ready 就放行。

响应丢失可查原 edit/receipt；晚到响应不得覆盖更新配置。关闭能力无需取得新的风险确认，但仍要版本校验和原子保存。

### 7.4 数据结构和约束

- Router 新增 capability policy/诊断投影及市场冻结快照存储；诊断缓存不作为运行时准入真值。
- Server canonical Share 与 grant 保存新策略，coverage acknowledgement 保存 scope、操作者、接受内容、条件指纹、生效与失效原因。
- 在现有 attempt/fact/task 上扩展能力策略版本与分维度证据，不重新生成 request/attempt ID，不新建独立会相互漂移的资源余额。
- 启用新策略后的最小事实包含持久 scope、实际执行模型、操作集合、原生数量/来源/精度、价目片段与分维度结算状态；Token/无限额调用也保留可取得的事实。没有 USD 定价时金额为空或部分已知，事实采集不因此停止。
- 核心 raw evidence、冻结价目片段、确认与回执按既有恢复/审计保留策略持久化，不能依赖短期 UI 日志保留期。
- 沿 `ServerStateInner` 域方法修改状态，遵守现有锁序与提交后发布；禁止 forwarder 直接写 Share 存储字段。
- 迁移与 reader 能力先就绪再启用新合同，迁移过程可恢复、重复运行不重复转移权限或消费。

### 7.5 错误契约

| HTTP / 类型 | 语义 |
| --- | --- |
| 403 `SHARE_CAPABILITY_DISABLED` | owner/grant 没有允许本次操作 |
| 400/422 `CAPABILITY_NOT_SUPPORTED` | 当前实现或模型不支持请求；不得自动切换成其他操作 |
| 409 `CAPABILITY_STATE_CHANGED` | 配置、绑定、范围或预览依据变化，需要刷新，保留草稿 |
| 409 `CAPABILITY_UPGRADE_REQUIRED` | 协议/执行版本/存储能力未就绪 |
| 409 `QUOTA_COVERAGE_REQUIRED` | 所选额度不能覆盖，且没有匹配的普通 scope 确认 |
| 409 `CAPABILITY_PREVIEW_EXPIRED` | 预览凭据过期或范围不匹配 |
| 409 `MARKET_CAPABILITY_CONFLICT` | 普通编辑会破坏活动市场合同 |
| 既有 USD/Token 拒绝码 | 额度不足、缺价严格拒绝、恢复 fence 等继续保留其原语义 |

错误附机器可读 operation、reason、适当脱敏的绑定/模型与 quota unit；不回传上游敏感原文。为 Claude/OpenAI/Gemini 分别映射原协议格式，WS 拒绝当前生成帧；流已开始后的失败按已有流错误通道记录。

## 8. Router 与 Server Web 的交互

### 8.1 Share 设置

独立「扩展能力」区域与额度区域并列，所有单位都显示。按当前绑定展示能力行，默认不勾选；未实现项不可选并说明原因，已保存但当前不可用的项保留意图和原因，不静默删掉。

每行至少显示：操作名称、可用范围、计量单位/精度、美元定价状态、对当前策略的额度影响。详细展开列出模型、缺价参数和未覆盖维度；不把 Profile/Driver 等内部字段放进普通主流程。

主行示例：

| 能力 | 用量采集 | 美元定价 | 当前 Token 额度 |
| --- | --- | --- | --- |
| 图片生成 | 可记录张数；无图片 Token | 已配置按张价格 | 仅交互 Token 计入，图片资源不计入 |
| 视频生成 | 时长为估算 | 已配置每秒价格 | 视频资源不计入 |
| 网页搜索 | 可记录实际调用次数 | 尚未配置 | 对话 Token 计入，搜索次数不计入 |

表内为展示语义示例，实际数据必须来自 Server，不写死某个供应商的绿色标签。

### 8.2 编辑用户授权与配额

显示用户有效能力，默认继承 Share，允许选择更小集合。切 Token/USD 仅重算覆盖说明，保留能力草稿；存在新缺口时在同一预览流程显示，保存不能悄悄禁用/启用能力。

普通 owner 选择未覆盖能力时展开本项说明与确认选项，确认不是隐藏 tooltip，也不能预选。批量编辑保持现有「仅修改已勾选字段」语义，权限改动和额度改动分别显式选择；各用户不同额度/历史问题分别展示，全批保存仍原子。

旧的「包含图片、音频、视频与付费工具」总复选框在 v13 UI 移除；旧协议兼容态可保留只读计价信息和升级入口，不假装有新操作开关。旧字段不能成为第二个可写权限源。

### 8.3 价格与用量明细

价格缺失指向当前身份有权使用的价格管理入口：资源价目前仅 Router owner 管理，Share owner 的文本补价权限不能被扩大。无权编辑时明确说明由谁配置；普通 owner 可按设计选择未覆盖例外，不能因此修改全局价。

用量明细展示真实 Token、图片张数、视频/音频单位、工具次数、冻结价格、各维度金额和估算标记。对于部分费用，显示「已计费 $X，另有 Y 未计价」，不显示完整总价 $X。

区分请求最终用户可见用量与多个实际生成 attempt 的收费证据。原始 usage 和聚合字段保持既有语义，不把内部重试重复加到客户端 final usage。

Server Web 通过本地 API 使用同一域判断，独立实现组件；与 Router 保持语义和契约夹具一致，不复制外部项目组件。

## 9. 旧配置、市场与升级迁移

### 9.1 兼容迁移采用两阶段

第一阶段只部署 reader、数据结构、执行器与诊断，现有 Share 保持明确的旧协议兼容态，不自动改变生产权限、历史金额或市场合同。

第二阶段由 owner 在完整预览后「采用独立能力控制」，一次原子保存进入 v13：

- 新 Share 的扩展操作为空；复制/导入新 Share 也默认关闭，只有显式选择复制权限并通过预览才继承已授权范围。
- 现有 Grok 三项明确 true 只映射当前对应绑定和相同操作，不扩散到其他供应商/App。迁移预览显示保留项与计量缺口；false 继续关闭。
- 旧 `tariffPolicyId=v2` 只证明采用资源价目，不作为所有工具/媒体的明确授权。
- 历史调用成功和“以前没有拦截”都不构成新许可；没有明确许可证据的扩展操作在采用预览中默认关闭。owner 可在同一操作中明确选择需要保留的能力。
- 无关描述/并发编辑不能偷偷触发采用。未采用 Share 显示「旧授权策略，扩展能力尚未独立控制」，不能展示为“全部已禁用”或“已完整保护”。
- 进入 v13 后缺少字段或能力丢失均拒绝降级，不能自动退回兼容行为。

这避免升级瞬间静默扩大或收窄大量旧用户权限，但旧兼容态并未获得新保护。灰度清单必须明确哪些 Share 已采用；生产完成验收要求目标 Share 全部采用或被明确保留在已知的旧行为中，不能把安装新版二进制等同于迁移完成。

### 9.2 市场合同

新 seat/quote/subscription/trial/grant 冻结有效操作集合、quota coverage 要求、计量精度/估算规则版本及价卡策略。报价与授权应用都重验相同条件，不能用普通 grant 的 zero 或风险确认放宽市场承诺。

普通编辑若收窄 Share 上限导致活动合同失去承诺能力，返回冲突并指向现有暂停/终止/到期路径。安全事故紧急暂停仍走现有管理流程并留下审计，不修改合同假装仍可服务。

旧市场租约保留原冻结语义；迁移预览无法证明其能力集合时，不用 owner 新配置追溯改租约。存在不兼容活动租约的 Share 暂缓采用，完成受控终止或到期后采用，再发布新 seat。可以明确证明等价的租约才做版本化的等价迁移，保留证据与合同 ID。

### 9.3 发布与回退顺序

1. 先发布 Router 的新 reader/协议支持，功能保持未激活；旧 Server 继续使用旧合同。
2. 发布 Server 的新执行器与 schema，完成现有存储 authority 就绪流程。
3. 验证双向协商、Server 控制执行、自有诊断及适用价目 active ACK。
4. 开放新建与旧 Share 采用入口，先灰度明确绑定和模型，再逐步扩围。
5. 活动旧市场合同单独处理；没有完整迁移条件不得扩大发布范围。

回退优先停用新建/采用入口，保持能读取 v13 的兼容执行版本继续服务和结算。不得删除新字段降成 v12、清空账本或通过恢复旧备份抹去升级后消费。真正恢复备份须使用已有完整数据恢复流程，并处理备份后的消费与任务，不属于自动代码回退。

## 10. 实施工作包与依赖

WP0—WP8 已完成代码、整体 review、修正、文档与对应本地验证；生产发布、真实灰度与逐 Share 采用不属于本轮本地完成状态。下表保留工作范围与依赖，实际结果见第 14 节。

| 工作包 | 工作内容与主要文件 | 完成门槛 | 依赖 |
| --- | --- | --- | --- |
| WP0 基线与合同清单 | 记录两仓库 HEAD/工作区；审查 16 个 Router 改动，拆出 `unpricedUsage` 依赖；核对 schema/reader 版本、真实已实现操作和入口；更新外部研究依据 | 每项功能有归属；现有状态与计划无混淆；操作/Driver/采集责任可追踪 | 无 |
| WP1 域模型与协议 | Server `domain/sharing`、`domain/providers/registry.rs`、`domain/usage`；Router `models.rs`、`share_value_pricing.rs`、`frontend/lib/types.ts`；确定 ID、集合语义、状态、错误和 v13 | 跨 Rust/TS 序列化、未知字段、空集合、旧 payload 指纹夹具通过 | WP0 |
| WP2 能力事实与预览 | Server 新建独立 capability 域/状态模块，复用账号 evidence；`api/control/ctl.rs`、`api/invoke`；Router `ctl_client.rs` 和私有预览 API | Token/USD/无限额均能取得精确范围诊断；无真实上游副作用；身份隔离和过期处理通过 | WP1 |
| WP3 持久化与原子保存 | Server `repository/server_sqlite*`、`state.rs`、Share canonical storage；Router 新 schema、`store.rs`；保存/回执/队列/确认审计 | 保存与准入竞态、批次失败、响应丢失、重放、重启后策略一致 | WP1、WP2 |
| WP4 全入口权限执行 | Server `proxy/request_governance.rs`、`proxy/forwarder.rs`、各 Driver/WS/内部生成；抽取集中 gate | 专用端点、内嵌工具、别名、映射、重试均不可绕过；多模态输入和客户端工具回归通过 | WP1、WP3 |
| WP5 分维度计量与结算 | Server `value_pricing/resource_request.rs`、`resources.rs`、`state/share_value_quota*`、`repository/server_sqlite/value_quota*`、usage store | 无整体清零、无重复计算、缺量不伪零、未知风险不释放、Token/USD 转换承接 | WP2、WP4 |
| WP6 Router/Server UI | Router `share-edit/*`、`lib/share-settings.ts`、价格/用量明细；Server `web-src/src/lib/api/share.ts`、Provider Bundle/Share 编辑器 | 独立配置、默认关闭、五类状态可读、确认范围准确、草稿/批量/切单位行为通过 | WP2、WP3、WP5 |
| WP7 市场与用量投影 | Router `share_market.rs`、`share_market/control_policy.rs`、`share-market/*`、`store.rs`；Server managed grants、事实同步与任务恢复 | 合同冻结、普通用户不可覆盖、严格范围拒绝、部分账单明细准确、异步不重复结算 | WP3、WP5 |
| WP8 迁移、整体 review 与发布材料 | 双仓库协议/文档/审计清单、迁移预览、兼容矩阵、灰度和回退说明 | 下述验收矩阵实际执行；未验证项如实标注；无 P0/P1 未解决项 | WP4—WP7 |

建议交付提交顺序：域/协议和兼容 reader，事实/预览，存储/保存，权限执行，计量修正，UI/投影，市场/迁移，整体 review 修正。对外激活必须等待完整链路；中间提交不得单独把新 UI 标为已可用。

Server 新模块优先拆到 `domain/sharing/capability_policy.rs`、`state/share_capabilities.rs` 等独立文件，避免继续将识别/价格/UI 诊断堆入 `state.rs` 或 `forwarder.rs`。具体新路径由 WP0 按当前模块布局确认。

## 11. 必须执行的验证矩阵

| 领域 | 关键用例与断言 |
| --- | --- |
| 默认与权限 | 新 Share 全关闭；新增目录项不自动开启；继承/显式空/子集语义；owner/free 用户/普通 grant/市场 grant；匿名与伪造测试 scope 拒绝 |
| 多模态输入 | 图片、PDF、音频、视频理解不被生成开关误拒；真实不支持输入按原模型错误返回；混合文本+媒体输出需要相应许可 |
| 工具 | hosted declaration、实际调用、仅历史结果、客户端 function 同名、namespace 注入/剥离、未知工具类型；未声明的协议控制字段不绕过 |
| 全入口 | Images generation/edit、Grok video、Responses/Chat/Gemini、multipart、无前缀别名、统一模型路由、native/cross-protocol、内部桥接和摘要 |
| WS 与续接 | 每帧新生成重验；旧 response 状态继承工具仍检查；切配置、关闭连接、HTTP fallback、重附着与真实重放正确区分 |
| Token 计量 | 图片/音频 Token 已接入时只算一次；无 Token 的媒体显示不适用；资源次数/时长不折 Token；aggregate 与细分无双计 |
| USD 计费 | 文本有价+工具缺价仍计文本；模型缺价不能绕过能力 gate；全缺价不是免费；显式零价；只有资源价的模型；参数限定缺价 |
| 估算与风险 | 请求界限不冒充观察；实际量大于估算不截小；完整证据才可下调；失败/取消/超时/未读 body 保留应有风险 |
| 数量与金额安全 | 负数、超大数量、非法枚举、NaN/溢出；JSON/multipart/透传/桥接的验证一致；拒绝结果不能产生负扣费或静默截小真实消耗 |
| 原子性与并发 | 配置关闭与新准入竞争；价格/模型/账号变动；最后额度多并发；批量任一用户失败全回滚；提交后响应丢失与重试 |
| 未覆盖确认 | 对 scope/用户/操作/单位/模型范围精确绑定；确认过期、重放、串用户、切单位、绑定变化、新缺口拒绝；不能绕过 reject、损坏存储或缺实现 |
| 异步任务 | 创建前预占、重启恢复、无人轮询、迟到完成/失败、重复回调、终态修订；关能力后仍可恢复旧任务；新 remix/extend 重验 |
| 历史与转换 | 能力变化不清零；100M→$100 承接历史；往返单位持续累积；资源无 Token 映射沿原校准；在途请求归原窗口；旧缺价按冻结规则 |
| 旧版兼容 | 新 Router+旧 Server、旧 Router+新 Server、双新未采用、已采用后 reader 退回/能力丢失；未知字段不能被旧 reader 静默丢弃 |
| 市场 | seat/quote/trial/subscription/grant 冻结一致；发布和租用时重验；普通编辑不能破坏活动合同；旧租约无法等价迁移时明确阻断 |
| 展示与隔离 | 所有单位都见能力；部分模型范围；缺价/免费/未知/估算可区分；未计价资源不被总价掩盖；登出/切号/迟到响应不串数据 |
| 持久化与恢复 | schema 升级幂等、未来版本拒绝、断电/事务故障、价目压缩后结算、日志清理后回执有效、scope 与账号 generation 隔离 |

验证分层：

1. 纯函数和契约 fixture：操作分类、状态计算、金额/数量归一化、序列化与迁移映射。以外部可观察行为断言，避免只匹配源码字符串。
2. SQLite/state 集成：准入、策略原子保存、在途结算、重放和恢复，故障注入必须有确定性的业务断言。
3. Router/Server 联调 fixture：真实协议格式、签名/回执、不同版本矩阵和 pending edit，不调用付费供应商。
4. 两端组件及浏览器：真实编辑流程、单位切换、批量和错误恢复，不只做静态标签断言。
5. 真实供应商灰度：每个准备公开的操作/Driver/模型组合独立证明执行、用量和拒绝行为；真实请求与生产发布另行授权。缺少输入继续 `live_pending`。

本地/CI 必须完成适用的 cargo 格式、check、实际回归、两端 typecheck/build，以及 Registry、UI provider、value-metering、协议、文档索引审计。Server 格式覆盖 include! 源文件；修改 Provider writer 证据文件时按 AGENTS 生成并校验 phase0 证据。只编译测试目标不算测试通过；不把静态 101 组合覆盖当作真实计费通过。

实施时至少安排以下现有检查，并增加本功能的行为测试到相应 suite：

- 两仓库 `cargo check --locked`；先执行受影响的 Rust 契约、账本、保存、转发测试，再完成仓库要求的后端回归。
- Router frontend：`npm run typecheck`、`npm run build`、`test:share-settings`、`test:share-value-quota`、`test:share-market` 及对应浏览器流程。
- Server web-src：`npm run typecheck`、`npm run build`，对 Share/Provider Bundle 组件使用已有 Vitest 测试入口。
- Server：`node scripts/audit/audit-provider-coverage.mjs --check`、`audit-ui-provider-matrix.mjs --check`、`audit-value-metering.mjs --check`、`audit-docs-index.mjs`；按变更范围完成 `scripts/static-checks.sh`、本地 smoke 与离线 release-readiness。
- 两端 v13/旧协议夹具、派生覆盖文件与签名规范审计必须进入正式测试入口，不能只在一次临时脚本中通过。

如果现有执行约定将某些产品回归留在 CI，报告其待执行状态；不主动轮询 GitHub Actions。真实灰度沿用现有要求（适用开放面 7 个自然日且至少 100 条完成请求），不能以本计划或本地 fixture 替代。

## 12. 整体 review 的交付门槛

整体 review 必须逐条追踪「用户选择 → signed patch → Server 持久化 → 最终 Driver 请求 → usage 事实 → 额度扣减 → Router 明细」，至少验证以下不变量：

- 任意价格解析失败、旧版字段丢失或自动重试不能绕过能力许可。
- 图片/文件/音频/视频理解输入不因生成许可关闭而整体失效。
- 新能力、新绑定、新未知维度不自动取得许可或未覆盖确认。
- 所有宣传为可计量/可计费/受额度约束的范围都存在实际采集、价格与准入执行证据。
- 部分缺价不抹掉已知费用，缺失用量不变成精确零，真实零价与未定价可区别。
- 新策略与单次生成按版本确定顺序，失败事务、重放和重启不产生半保存、重复扣费或丢失风险。
- 关闭能力不破坏旧任务对账；市场合同和周期历史不被普通编辑改变。
- 未实现、未验证、兼容态和真实灰度待验收在 UI、API、文档及报告中保持一致。

完成代码实施时交付：两仓库具体变更、实际执行的验证及结果、未完成真实验收项、review 问题与修复记录、采用/回退步骤。不把“owner 可以明确承担未覆盖消耗”描述为完整计费支持。

实施结束后的 commit/push 仍依用户当时授权；Router push 必须使用 `./git-push.sh`，不直接执行 `git push`，不为 wrapper 暂存无关改动。

## 13. 文档与外部项目证据的使用

实施时同步 Router `PROTOCOL.md`、原 `docs/share-user-value-quota-implementation-plan.md` 的有效语义，以及 Server `docs/share/universal-value-metering.md`、`value-quotas.md`、`quota-unit-conversions.md`、`architecture/usage-accounting.md`。旧“资源总勾选决定许可”的叙述标明被新协议替代，保留历史记录。

Server 若新增独立能力规范，登记到 `docs/README.md` 并标注状态；新增 UI 命令更新 `assets/contract/web-runtime-contract.json`。根据外部证据调整 Provider 行为前，核对/更新 `PROTOCOL_EVIDENCE.md` 与 `docs/provider/coverage.md`。

以下仅为本次设计的只读研究依据，不成为构建、测试或运行时依赖，不批量复制外部代码/UI：

| 项目、本地基线 | 借鉴点 | 证据文件 |
| --- | --- | --- |
| sub2api `ab99d56e9` | 默认关闭的独立生图许可；专用/内嵌操作识别；媒体原生单位；异步价格快照与幂等结算 | `backend/ent/schema/group.go`、`backend/internal/service/image_generation_intent.go`、`billing_service.go`、`batch_image_public.go`、`batch_image_settlement.go` |
| TokenRouter `d15137eb5` | 后端能力目录、分组许可与供应商路线分离、未知/显式空集合、关闭新生成与旧任务生命周期分离 | `docs/interfaces/protocol_capabilities.md`、`backend/internal/routing/capability/{protocol,group,route}.go`、`backend/internal/modelcatalog/capabilities.go` |
| new-api `2d8e50bf` | 用量归一化与避免重复计费、实际工具事件计数、图片流事件去重、发送前价格检查 | `service/billing_usage.go`、`service/tiered_settle.go`、`relay/common/tool_usage.go`、`relay/helper/price.go` |
| LiteLLM `9a715df212` | 输入/输出能力分开、模型/操作相关价格单位、Token 化图片的专用计费器 | `litellm/types/utils.py`、`litellm/llms/openai/image_generation/cost_calculator.py` |

不照搬的行为：媒体缺价使用无关模型通用默认价、缺 usage 默认为零成本、预算无法估算就被当作完整保护、把内部计价配额的 TOKENS 展示当作真实 Token 限额。静态能力表、价格目录和一次成功请求均不能证明所有模型、所有流式/异常路径已具备完整计费能力。

## 14. 实施与整体 review 记录（2026-11-17）

### 14.1 交付范围

| 工作包 | 实际落地 |
| --- | --- |
| WP0 | 保存两仓基线；保留 Router 起始 16 个修改文件；核对已实现 Driver/入口、账号证据、schema 与 reader；未引入外部运行依赖 |
| WP1 | 双端 10 个稳定操作 ID、v13、继承/显式空/子集语义、错误码和相同契约夹具；Server `domain/sharing/{capabilities,capability_intent}.rs` 与 Router `src/share_capabilities.rs` |
| WP2 | Driver/Registry 实际操作事实、只读预览、15 分钟签名、运行时/用户/单位/缺口作用域；Router Dashboard、独立 Share、Server Web 三入口 |
| WP3 | Server schema 8 四表、Router migration 0117；普通/批量/native 保存同事务提交策略、确认和永久回执；重复操作与已删除 Share 不复活 |
| WP4 | 权限位于价格解析之前，覆盖 HTTP/SSE、媒体、WS、HTTP fallback、Driver 内部生成与新重试；owner、Token、USD 都执行 |
| WP5 | 部分资源缺价继续结算已知费用；缺数量不当零；保留原 request/attempt、窗口、冻结价、人工调整和风险；续接不重复继承旧理解输入 |
| WP6 | 双端独立能力编辑器，绑定选择、grant 继承与批量收窄、逐 scope 覆盖确认、缺价/免费/未知/估算、隔离和冻结用量展示；旧资源总开关只读 |
| WP7 | 新 Market schema 3 明确冻结空扩展集合及 `text-tokens-v1`；报价漂移拒绝，买家不继承以后新增能力；非管理者只取得本人有效操作投影 |
| WP8 | 完成配置→签名→持久化→发网→事实→额度→投影的整体 review 和修正；本地最终回归结果见下；真实验收/部署另列 |

实际调整及边界：新 Share 先全关创建，再按真实绑定预览；首版 Market 只出售基础模型交互；未实现的 hosted computer/remote MCP 不开放，video 仅使用已实现的 Grok 路径。原生资源数量可能估算，当前 Token/USD 都不能被描述为对所有媒体有硬预算保证。有限 Token 的资源缺口和普通 USD zero 的缺价，均须 owner 逐项确认；USD reject 不接受此豁免。

### 14.2 整体 review 的问题与修正

1. 修复能力 admission 使用只读数据库连接的错误；同 request/attempt 重放核对完整 scope、binding 和计划，不能覆盖其他用户/Share 的上下文。
2. 权限检查移到价格解析之前；纯文字也核对真实运行时指纹。新增绑定、身份 generation、模型/Driver 规则变化不自动获准；旧 reader 不能靠丢字段降级。
3. OpenAI image action auto/省略检查生成和编辑；Gemini 图片输出加素材检查两项，显式 TEXT 允许理解。普通同名 function、schema、prompt 和历史结果不误判为生成。
4. 未知 hosted 工具、未知输出模态和已知 Gemini 工具旁混入的未知字段在发网前拒绝；同一 Gemini 工具对象的多个已知操作全部检查，不能只检查第一项。namespace 与客户端 tool_choice 避免重复复制大型 schema。
5. WS 保守继承工具/非法控制状态，每次新生成重验；HTTP previous_response_id 只恢复相同 Router/installation/Share/用户/App/指纹。续接仅带执行资源，不重复扣上一轮音频理解输入。
6. 未计划的图片 Token、音频毫秒等输出单位也记录 incident；异常按用户/App/binding 隔离。后续修订不能把别人的 scope 带入事实；不提供删证据绕过隔离的按钮。
7. 文本有价、某资源缺价时保留文本费用；只有资源价而文本 tier 缺失时保留资源费用。缺量、取消、未读 body 和失败沿原保守风险规则，不伪造成免费。
8. 能力保存增加同事务永久回执；先校验重放再检查过期凭据/CAS。坏 HMAC/hex 返回结构化过期错误；删除 Share 后重放不能成功。
9. 能力错误码贯通 Claude/OpenAI/Gemini、Web 管理和 Router 预览；隔离错误不可被客户端当作临时上游失败自动重试。
10. 修复 Router 存储中历史 `share_access_policy_version=0` 被误当线协议版本的问题：有能力策略重建 v13，无策略保留 v12；心跳 reader 为 13。
11. 双端拒绝迟到身份/Share/revision 和非法结构的预览；修改单位、用户策略或能力后使旧确认失效。显示继承授权数量，已保存但不可用意图不静默删除。
12. Token/owner/无限额没有 USD attempt 时显示匹配 attempt 的真实原生数量，金额为空；不使用当前价重算、不将另一次重试的数量附给旧 attempt。
13. 生成的 Provider coverage 说明移回官方生成器，writer 清单由官方脚本生成；价格目录测试核对实际 4 MiB 协议上限，没有扩大生产限制。旧 API 转发夹具通过正常启动流程提交 SQLite authority，并先保存账号再写 Provider 外键。
14. 补齐 CI 对新增能力回归的调用：Router 执行共享策略合同和能力浏览器测试，Server 执行 `capability` Rust 测试；不只依赖本地验证或旧金额测试过滤器。

本地代码 review 未发现仍未处理的阻断项；这不证明真实供应商全部支持或线上账单已完成对账。

### 14.3 验证记录

最终本地验证结果如下；不将单项复测描述为再次运行全量测试：

| 验证 | 实际结果 |
| --- | --- |
| Router Rust | `cargo check --locked`、测试目标编译通过。全量运行 2,005 项通过，3 个旧版本夹具失败；修正这 3 项后逐项复测全部通过。另有 9 项 capability 过滤回归通过，CI 使用的 `share_capabilities` 合同入口也通过 |
| Router 格式 | 14 个本轮变动的 Rust 文件（含 `include!` 文件）按仓库 edition 2024 检查通过。全库 `cargo fmt --check` 仍报告 `alerting/channels.rs`、`alerting/store.rs`、`notification_compose.rs`、`user_notifications.rs` 的既有格式差异；这 4 个文件相对 HEAD 无修改，本轮没有改动它们 |
| Router frontend | 类型检查；Share settings 23、value quota 46、Market 85 项测试通过；既有设置浏览器与新增能力浏览器流程通过；12 项 Node audit 和价格/i18n/settings/billing 审计通过 |
| Router 生产构建 | 最新 UI 在官方字体 mock + Webpack 下完成生产构建、27 个静态页生成；默认 Turbopack 因 Google Fonts 外网超时未完成，未修改产品字体或关闭 TLS |
| Server Rust | `cargo check --locked` 和二进制构建通过；全量测试 4,499 项单元测试、139 项 API 合同和 2 项其他集成测试通过，共 4,640 项；3 项显式 opt-in 用例忽略。新增 CI `capability --lib` 入口实际执行 64 项测试，全部通过 |
| Server Web | 58 个文件、407 项 Vitest 通过；类型检查和生产构建通过，`web-dist` 已同步 |
| Server 静态与 smoke | 完整静态检查通过（含严格 Clippy、2,205 项 Node audit、13 项 smoke 合同、Registry/UI/docs/格式/依赖/状态写入审计）；本地启动、鉴权、Provider 与 Share 创建冒烟通过 |
| CI 与跨仓检查 | 两个修改的 workflow YAML 解析通过；Router 使用正式 timed/require-tests 包装验证合同和能力浏览器入口；两仓 `git diff --check` 通过 |
| 离线 readiness | 按仓库约定运行 `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh`，`failures=0`。脚本按设计报告 `blocked_inputs`：同次 readiness 未内嵌重跑测试、缺 8 项真实环境输入及未部署。上列测试已分别实际执行，但不能因此将该脚本输出改称可发布 |

Server debug 异步夹具按已有 CI/validate-local 约定使用 `RUST_MIN_STACK=67108864`；Node 26 的 JSDOM 测试使用 `NODE_OPTIONS=--no-experimental-webstorage`。未改产品鉴权来适应测试环境。显式 opt-in 的真实 Ollama、十万 attempt 容量和 Codex loopback 基准不包含在常规测试通过数量内。

### 14.4 发布与真实验收

本地实施不代表生产已经采用新策略。需按 §9.3 先 Router、后 Server，确认 reader13、能力协商、committed schema8 及适用价目 active ACK，再逐 Share 预览采用。旧合同、缺实现、计量未知与缺价必须保持明确诊断；修复运行时后重新预览新指纹，保留旧用量、永久回执与异常证据。

本轮没有提交、推送、部署、远程修改或真实付费上游调用，也没有等待 GitHub Actions。离线 readiness 可以证明本地发布材料检查结果；缺真实输入、部署和灰度对账时仍是 blocked/live_pending。回退必须保留可读取 v13/schema8 的执行版本，不删字段、账本或用旧备份抹去新消费。
