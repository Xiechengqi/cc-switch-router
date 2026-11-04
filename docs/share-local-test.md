# Share 连接弹窗：本地测试

Clients 页的 Share「连接」弹窗保留原来的 Router 端「测试」，并在旁边提供「本地测试 ↗」。确认外部站点的 Key 风险后，在独立新标签页打开 `https://llm-api-test.xiecq.top/`，原页面不导航、不自动调用原测试接口。

## 参数与协议

- `api`：该 Share 的 HTTPS 根地址 + `/v1/chat/completions`，不是统一 `api.*` 入口，也不是 Provider 上游地址。
- `key`：当前登录用户 `/v1/me/api-token` 返回的完整 API Key，不是登录 Token、Share 内部密钥或遮罩前缀。
- `model`：当前行的 `modelProbe.requestedModel`，与「测试模型」显示一致，不替换成 `wireModel` 或固定模型。

目标工具目前发送 Chat Completions 请求体，首期只启用 Codex/OpenAI 文本测试。Claude、Gemini 和图片／视频操作保留原测试入口，并提示本地测试不支持；不能把 Responses、Messages、Gemini 的路径直接传给只发送 Chat 请求体的工具。

参数通过 `URL` / `URLSearchParams` 编码，只在明确确认时构造完整链接。新标签页使用 `noopener,noreferrer`，不向 Router 日志、错误提示、持久存储或页面 href 写入敏感链接。`window.open` 在 noopener 模式返回 null 并不证明窗口被阻止。

## Key 与会话安全

用户 Key 是账号级凭证，不仅能调用此 Share。确认框明确提示：查询参数会发送给外部站点，可能被其访问日志或浏览器记录；目标页随后清理地址栏不能消除首次发送的风险。「本地」指用户浏览器发起请求，不代表离线或免费；模型调用正常消耗额度并计费。

凭证状态按账号与登录轮次隔离。关闭、退出、切换账号时不展示旧 Key；取消读取、丢弃迟到响应；401 不跨账号重试。相同登录内的 access token 轮换不重置登录上下文。完整 Key 不可用时禁用入口，读取失败可重试。确认打开时再次核验当前登录身份。

## Share API CORS

Router 仅对已识别 Share Host 的公开模型 API 与模型列表提供无 Cookie 的 Bearer CORS，沿用统一模型入口的头部规则。Router 管理端点、Share 控制面、Client Web 不继承此能力。

合法 OPTIONS 预检直接返回 204，不要求 Key、不记录无效鉴权、不调用上游、不占模型并发、不计量。实际请求继续执行原有 Key 鉴权、Share ACL、模型限制、配额、限流及计费；错误响应也携带 CORS 头。离线 Share 通过持久化 Host 识别，不能误把 Client Web 当作 Share。

## 验证

`npm run test:share-model-probe` 覆盖原 curl、外部链接参数与编码、协议限制、Key 不可用状态、新标签页隔离及会话切换；Rust 的 `browser_share_cors` 用例覆盖真实 HTTP 预检与实际鉴权、跨域范围、离线 Host、IP 黑名单和无上游调用。另执行类型检查、构建和相关回归测试。

所有测试只用虚构凭证或 loopback fixture；不截图、不运行 UI/浏览器自动化、不访问带真实 Key 的链接、不调用真实模型。
