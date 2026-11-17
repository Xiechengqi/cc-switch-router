# 钱包安全修复整体 review（2026-11-17）

范围为迁移 114–116、认证/隧道边界、充值、C2C、邮箱转账、通知、对账、恢复 CLI，以及对应前端。现行协议与部署步骤见 [钱包安全与异常恢复](market-wallet-security.md)。此次仅修改本地工作区，没有执行生产付款或远程部署。

## Review 中发现并修正的问题

| 问题与影响 | 修复及验证证据 |
| --- | --- |
| 设备签名中的令牌摘要与存储编码不一致，正常浏览器资金请求会被拒绝 | [market_security.rs](../src/market_security.rs) 按协议计算 Bearer 的十六进制 SHA-256，保留数据库 Base64 存储格式；`financial_http_signatures_fence_tampering_replay_and_share_control_hosts` 使用真实令牌解析和 HTTP 请求验证成功、篡改、重放及 Share Host 拒绝 |
| 动态资源路径可能与风险分类规则不一致 | 风险判断解码动态 ID，签名仍绑定原始 URI，安全 ceremony 的豁免只匹配其准确前缀；`encoded_order_ids_cannot_skip_large_release_confirmation` 验证等价编码路径及管理端 `security` 资源名不会选中豁免 |
| 并发完成首次登记后，另一个已通过首次登记预检的请求可能继续添加凭据 | Passkey 登记最终事务重新检查现有凭据，追加凭据必须持有已确认的请求证明；`a_revoked_passkey_or_reused_grant_cannot_authorize_money` 覆盖无确认的并发登记证明、凭据撤销和 grant 复用 |
| 全部旧 Passkey 撤销后，本地恢复仍可能被旧登记历史要求的 Passkey 确认阻断 | 恢复登记只以未撤销凭据判断追加确认，历史存在时仍必须具有本地恢复许可；`local_recovery_requires_fresh_email_proof_revokes_other_sessions_and_replays_once` 覆盖恢复、其他 Session 撤销、旧 ceremony 失效及恢复重放 |
| 三次未付款触发后，第一条记录滚出当日窗口会提前结束冷却 | 冷却按最后一次满足三次条件的触发时刻持续完整 24 小时；`unpaid_cooldown_lasts_a_full_day_after_the_qualifying_strike` 验证边界 |
| 回调锁外验签成功后等待数据库锁，旧密钥宽限可能已过期 | 最终事务同时复查 revision 和当前宽限时间；`webhook_key_grace_is_rechecked_after_waiting_for_the_commit_lock` 覆盖等待越过期限及紧急轮换 |
| 新迁移与旧升级测试中的表数量/会话断言冲突 | 更新当前表数量，明确迁移 114 的一次性撤销规则，并将迁移 98 不新增财务表的断言限定到该迁移；`migration_114_revokes_existing_sessions_and_rotation_grace_only_once` 验证重开数据库不撤销新登录 |

## 资金与恢复不变量

- 所有隧道剥离 Cookie，上游 Set-Cookie 不回传；浏览器独立凭据不能作为 Bearer。除逐头检查外，还用真实 Share 上游验证双向边界。
- 授权、金额、版本、限额、余额、grant 消费、复式 journal 与通知在最终事务内检查/提交。资金失败会回滚确认消费；同一业务重放不能再次扣款。
- 充值缺失数量不跳过校验。聚合超限保留真实支付事实，熔断持久化；RPC 查询和回调解密验签不占用数据库锁，最终写入复查配置。
- 买家沉默不能证明未付款。补证超时转独立裁决；限制误判通过追加证据更正，原裁决和终态账本保留。
- 退回只由原收款人确认。余额不足或有优先服务义务时不扣款；并发批准只产生一个补偿 journal。管理员只能协助发起申请。
- 对账 cursor 持久化并环回，首批长期异常不会阻挡后续检查；历史授权证据不随 Session 或无用证明清理而删除。

## 验证

- 最终市场回归：`cargo test --locked --bin cc-switch-router market_ -- --test-threads=4`，578 项通过。
- 最终数据库迁移回归：`cargo test --locked --bin cc-switch-router schema::tests -- --test-threads=4`，82 项通过，覆盖新库、旧库升级和迁移重放。
- 生产代码：`CARGO_INCREMENTAL=0 cargo check --locked -j 2` 通过；保留仓库原有未使用代码告警。
- 前端：类型检查、构建、钱包单测 60 项、密码认证单测 10 项、设置 i18n/账务契约审计通过。
- 浏览器：真实 WebCrypto/虚拟 Passkey 验证签名字节、UV、原始正文/幂等键及新 nonce；钱包页面覆盖风控 CAS 冲突恢复、Passkey 撤销、转账申请和收款人确认；C2C、Client checkout/termination/rental audit、导航余额、密码及通知回归通过。
- CI 契约测试 6 项通过；新增 HTTP/凭据边界及 Passkey 浏览器检查已接入 CI。
- 本次独立修改/新增的 37 个 Rust 文件通过 `rustfmt --check`，`git diff --check` 通过。全仓 `cargo fmt --all -- --check` 仍报告既有告警/通知文件和本轮开始前已存在的 Share 额度改动的格式差异，未批量改写这些内容。

修复过程中启动的全量后端回归已结束：2,000 项中 1,987 项通过、13 项失败。该运行使用修正前的测试二进制；其中 2 项 HTTP 集成失败、10 项迁移兼容断言失败均已修正，分别通过最终 HTTP 与完整迁移回归。修正后的最终测试二进制包含 2,003 项，未重新执行整套回归，以上分别列出实际完成的复测范围。

剩余一项独立复现的既有失败为 `model_price_catalog::tests::execution_snapshot_is_the_displayed_price` 容量断言：当前快照 2,229,549 字节，超过测试要求的半上限 2,097,152 字节，仍小于实际快照上限 4,194,304 字节。该问题此前已记录在 [请求体超时 runbook](runbook-ingress-body-timeouts.md)，本次不改动模型价格目录或定价契约，不能把结果表述为全量全部通过。

## 仍需部署验收的边界

迁移 114 会让现有用户重新登录。初次高危资金操作须先登记 Passkey，并等待 24 小时或经部署管理员核实后批准指定凭据。独立链上验证需配置受部署权限保护的 RPC/网络/资产/收款地址锚点，并逐轨道进行真实商户验收；本地 mock 不能替代。设备私钥仍与令牌同存浏览器，签名不能抵御主站 XSS 或完整设备失陷。
