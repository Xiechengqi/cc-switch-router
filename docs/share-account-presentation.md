# Share 账户信息的统一展示

Client 与 Share Market 卡片共用 `shareAccountPresentation` 和 `ShareProviderStatusPanel`。
市场身份摘要使用同一个账户视图构建函数及 `marketCapabilityRuntime` 适配器。
供应商分类、额度摘要、Buddy 签到合并与批次明细继续集中在已有额度格式化模块。

主账户按显式 appType 选择（必须启用且存在），否则按 Claude、Codex、Gemini 固定顺序选择。
不得因另一个 App 恰好报告更多额度字段而切换主账户。各页面保留自身健康状态和模型列表的业务语义。

公开额度采用服务端字段白名单，保留资源包来源、到期时间、额度池元数据、活动费用及有效的 Buddy 官方账户用量摘要。
不公开内部包名、credentialMessage 或 bankedReset；账户身份继续由现有服务端授权和脱敏逻辑决定。
公开 API 供应商显示名称，不公开请求地址。缺少字段表示未知，不从显示名称猜测资源包来源，不将重置时间当作到期时间。
旧服务端缺少元数据时仍使用保守展示，升级服务端后恢复完整汇总。

## 验证与 review

回归覆盖包括公开 DTO 字段保留与私有字段排除、Buddy 两产品的签到合并和批次明细一致性、
多个供应商的额度适配保留、多 App 主账户稳定选择；保留已有身份脱敏和 bankedReset 不公开的测试。
本次按既定约束仅进行 TypeScript 类型检查、Rust 测试编译和静态检查，未执行产品测试或线上请求。
CI 回归执行结果仍待确认。

本地检查结果：`npm --prefix frontend run typecheck`、`cargo check --locked --tests`、
定向 rustfmt 检查及 `git diff --check` 均通过。Rust 仍有其他文件的既有 unused/dead-code 警告。
整体 review 已核对公开字段白名单、账户选择、共享调用入口及现有 CI 测试筛选范围；
新增用例分别进入既有 `test:share-market` 和 `share_market::` CI 测试组。
未发现本次变更中的阻塞问题；运行期回归结果仍以 CI 为准。
