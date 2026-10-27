# 市场一次性试用规则

## 配置与冻结

Router owner 在「设置 → 市场 → 平台钱包 → 市场试用规则」设置 Client 默认小时、Share 默认小时／一次性 Token。初始值分别为 12 小时、12 小时、1,000,000 Token；小时范围 0–720（整数），0 小时关闭试用，0 Token 为不限 Token 的时间试用。Token 输入按 M 展示、按非负安全整数持久化。

Host 可覆盖时长；Share 车位可分别覆盖时长和 Token。空值继承，明确的 0 不继承；历史车位明确存储的值不改为继承。废弃的浏览器默认条款不再读写。Quote 冻结有效规则、来源、版本、完整额度和剩余额度；改变全局默认不改写有效 Quote／合同，不提升钱包维护费版本。策略更新采用 expectedVersion 并记录前后值和操作者。

- 公共读取：`GET /v1/market-trials/policy`。
- 管理员读取／Router owner 修改：`GET/PUT /v1/admin/market/trial-policy`。
- 迁移 92 增加 Client 配置与冻结字段，93 将试用策略从钱包策略中分离，增加 Share 一次性 scope／合同水位。迁移不改写余额和既有合同价款。

## 准入与一次性预算

所有付费服务含试用均要求完整首期可用余额；试用期不预留、不扣费。免费商品和 Provider 自用仍走原免费逻辑。试用不等于允许零余额占用付费资源。

Share 按 buyer_user_id＋provider_user_id＋share_id 合并历史健康服务时长和 Token，释放、换车位、重启不重置。时间或 Token 先耗尽即结束，同一个 scope 永久标记结束，后续增大默认值不能复活。旧的已转付费合同也不重新提供试用。普通日／周／月额度仍独立生效，不可拿周期重置当作新试用。

Server 在转发前持久化 request ID 对应 entitlement 与 phase。累计 revision 幂等、只增不减；旧 entitlement 的迟到 Token 继续减少整个 scope 的剩余预算，不能进入新租约或正式阶段 quota。并发中已准入请求可能在阈值之后完成；拒绝新的请求，不凭空撤销已产生的实际消费。

## 转付费与安全回收

Share 使用 `trial → activating → paid`：耗尽后先预留首期价款，下发正式授权，校验 Server 确认的正式 phase、应用时间和冻结 policy，然后捕获资金并从正式应用时间起算付费和额度。时间／Token 同时触发不重复预留或扣款。

首次支付余额不足、授权明确拒绝、错误正式回执或不可证明完整的用量，进入 `revoking`／合同 `ending`。未知授权回执超过 5 分钟开始撤权，但保留资金预留直到确认清理。之后补款不会恢复已回收流程。Client 的回收必须清掉进程、租客身份及数据；Share 必须确认撤权。未确认清理的 Host／车位不能重新出租。

Server 的未完成请求若跨进程重启，用量无法从权威日志恢复时上报 `usageUncertain` 并禁止继续使用；Router 安全回收，不视为零用量、不自动产生一笔首期扣款。已完成精确 request receipts 保留 35 天（超过日志 32 天修订窗口），之后压缩进不可归零的 lifetime 累计；pending receipt 不丢弃，最近 receipt 达容量上限时 fail closed。存储失败同样拒绝后续市场流量。

免费预算异常隔离依据请求发起时的 trial 阶段，包含转正式后才返回的试用请求。普通 paid 请求取消仍按既有付费额度规则计量，不会因此触发试用回收。

旧租约释放后才发现未知试用用量时，隔离整个 eligibility scope，包括同 scope 已经开始的后续租约；不能只修改已结束的旧合同，而让新合同被扣款却无法使用。

## 升级与验收

新 Share 试用要求 `Share Contract v9`＋`marketTrialV1` 显式协商。普通未协商 descriptor 仍为 v7，市场整体最低版本仍 v5；旧 Server 的免费／关闭试用报价不受影响，真正试用报价明确提示升级，不降级成周期额度。

建议先部署支持本协议的 Server（对旧 Router 仍发送普通 v7），再升级 Router 并等待 heartbeat／descriptor 协商到 v9。已有 v9 entitlement 时不能无准备降级 Router；Server 会拒绝向无能力的 Router 发布该状态，不静默丢弃试用身份。上线前备份两个数据库／数据目录。

本地验收覆盖边界与继承、冻结报价、试用首次余额、跨周期／重启／迟到回执、一次扣款、未知回执保留资金及清理后释放。浏览器使用 mock API 和 DOM 断言，不做 UI 截图；真实部署、真实充值和网络联调另行验收。
