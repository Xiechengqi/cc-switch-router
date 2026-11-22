# 平台钱包安全与异常恢复

本文为迁移 114–116 及 121 后的当前契约，覆盖官方充值、C2C 和邮箱转账。早期文档中的“仅凭登录会话即可操作资金”已被替代。历史账本、OTP 证据、Passkey 证据和已成交订单不改写。

## 固定规则一览（迁移 121）

用户不配置任何安全选项，下列规则由服务端统一执行：

| 规则 | 取值 |
| --- | --- |
| 需要确认的操作 | 转给新收款人；单笔 ≥ 500 美元转给已知收款人；C2C 放行 ≥ 500 美元；Owner/管理端财务写入。充值和消费永不需要 |
| 确认方式 | 登录密码（每次操作一次，绑定本次请求）。没有密码时引导设置 |
| 新收款人 | 转出人此前没有向对方转过、且已到账的钱（只看转出方向：收到过对方的钱不算，防止他人先转一分钱“预热”受害账户）。转给新收款人先冻结，24 小时后自动到账；到账前转出人可单方撤回。报价时显示为待到账的，确认时仍按待到账处理 |
| 改密冷静期 | 通过网页设置或修改密码后 24 小时内，密码不能确认“立即生效”的操作（≥ 500 美元已知收款人转账、≥ 500 美元 C2C 放行、Owner 财务写入）；新收款人转账不受影响，因为它本身要等待 24 小时。部署管理员在 Router 主机上用 `cc-switch-router user password set` 设置的密码立即可用（主机 root/服务用户本就是最高信任），用于首次上线时 Owner 无需等待 |
| 到账后 | 不可撤回；收款人可在账单里“原路退回”，即一笔普通转账，同样遵守新收款人规则 |
| 到账失败 | 到期时收款人或转出人已被禁用、或转出人余额已不足以覆盖冻结（如充值被冲正），自动退回转出人并通知 |
| 紧急暂停 | Owner 仅“暂停”新订单/新充值时只需密码、不受改密冷静期限制；任何可能解除暂停的请求仍属立即生效操作 |

本轮发现的问题、修正证据与测试范围见 [整体 review](market-wallet-security-review.md)。

## 身份边界

- 所有 Share、Client Web 和 Gateway 隧道请求剥离 Cookie；Share/Gateway 同时剥离调用方 Authorization、API Key。上游 Set-Cookie 不回传浏览器。
- HTTPS 浏览器凭据为 `__Host-cc_switch_router_session`，不带 Domain，具有 Secure、HttpOnly、Path=/、SameSite=Lax 属性。仅 localhost 开发环境使用不带 Secure 的 `cc_switch_router_session`。
- Cookie 是独立随机 `browser_…` 凭据，数据库仅存其哈希，不能作为 Bearer。它随原登录会话撤销、轮换和过期失效。
- `/v1/auth/*`、`/v1/admin/*`、`/v1/market-wallet[/…]`、`/v1/market-provider-finance/*` 只在 Router 主 Host 提供。Share Host 上返回 404。
- 财务写入必须同时持有真实 Session Bearer 和本次登录设备的签名。Cookie、API Key、开发模式身份、单独新注册的设备不能代替。提供 Origin 时必须精确匹配部署配置；浏览器跨站请求被拒绝。
- 最终 IMMEDIATE 资金事务重新检查 Session 撤销、访问/刷新有效期、用户状态、auth_version、设备撤销和请求证明。网络查询前的预检不能代替最终检查。

设备 Ed25519 私钥与登录令牌仍在现有浏览器本地存储模型中。设备签名证明请求持有设备密钥，并防止单独盗用 Bearer；它不是独立 MFA，不能抵御 Router 主站 XSS、恶意浏览器扩展或完整设备失陷。

## 请求签名与密码确认

财务写入前缀为 `/v1/market-wallet/`、`/v1/admin/market-`、`/v1/market-provider-finance/`。公开支付回调不在这些前缀下；Owner 的 `webhook-secret` 轮换不能豁免。

签名字节为 UTF-8，下列字段用换行连接，不追加末尾换行：

```text
market-finance-v1
Router 配置的 origin
auth_device_id
sha256(access_token)，小写十六进制
HTTP METHOD
原始 path?query
sha256(原始正文)，小写十六进制
Idempotency-Key（没有时为空）
timestamp_ms
nonce
```

请求头为 `X-Router-Device`、`X-Router-Request-Time`、`X-Router-Request-Nonce` 和 `X-Router-Request-Signature`（标准 Base64 Ed25519）。时间容差 5 分钟，同 Session nonce 唯一，正文最多 128 KiB，每用户每分钟最多 120 个有效请求。内部 proof header 由 middleware 生成，外部传入值先删除。

需要确认的操作（见上表）由服务端以 428 返回 `{code:"FINANCIAL_STEPUP_REQUIRED", details:{challengeId, method:"password", risk:"confirm"|"immediate"}}`。前端弹出密码框，经设备签名的 `POST /v1/market-wallet/security/confirm {challengeId, password}` 校验后，以 `X-Router-Stepup`、原始正文、原业务幂等键和全新 nonce 重发。

- 确认记录在 `market_password_confirmations`，绑定用户、Session、请求签名摘要（路径、正文、幂等键）与风险等级，5 分钟有效、一次执行消费；业务事务失败时消费随之回滚。
- 密码校验与登录共用 `auth_password_attempts` 持久化预算（邮箱+来源 10 次/15 分钟、单邮箱 100 次、来源 60 次、设备 20 次），Argon2 在数据库锁外执行，前后各检查一次 Session。
- `risk=immediate` 的确认在最终业务事务里再检查一次密码年龄：确认之后又改密的，确认作废。改密本身会撤销所有旧 Session，因此改密前的确认也随 Session 失效。
- 没有密码：返回 403 `FINANCIAL_PASSWORD_REQUIRED`，前端打开“设置登录密码”。冷静期内：403 `FINANCIAL_PASSWORD_COOLING`，`details.usableAt` 给出可用时间。
- 每用户每分钟最多 12 个确认请求。
- 财务写入正文必须是 JSON 对象；风险分类读取的字段与业务处理一致，数组等位置参数形式直接 400。转账业务事务内还会独立核对本次请求确实带有足够等级的密码确认（待到账需 `confirm`，≥ 500 美元即时到账需 `immediate` 且不在冷静期），中间件分类出错也不会放行资金。

### 为什么用登录密码而不是 Passkey 或邮箱验证码

任何有效 Session 都可以设置密码（见[密码登录](password-authentication.md)），所以密码本身不能防“偷到 Session 的人先改密码”。防线是两条固定规则：新收款人 24 小时待到账（期间可撤回、改密和转出都会通知邮箱），以及改密后 24 小时内不能确认立即生效的操作。受害者用邮箱验证码重新登录并改密，就会撤销攻击者的全部 Session，无需管理员介入。邮箱验证码不作为每次确认手段，避免邮件投递不稳定直接阻断转账。

残余风险：主站 XSS 能截获输入中的密码（Passkey 不会被导出）；设备私钥与令牌同在浏览器存储中，签名不能抵御完整设备失陷。

### Passkey 退役

迁移 121 起不能再登记 Passkey（`market_passkeys` 插入触发器拒绝），`/v1/market-wallet/security/passkeys/*` 与 `/authenticate` 返回 410。既有凭据、ceremony 与 `market_security_events` 只读保留，供审计。命令行 `user finance-passkey` 已退役；忘记密码的用户用邮箱验证码登录后重设，管理员可用 `user password set`。改密确认邮件不再包含密码明文，并提示 24 小时冷静期；迁移前已排队、密封了明文的通知在投递时也不再渲染密码。

## 充值验证、限额与熔断

回调只唤醒认证查单，不直接创建钱包余额。法币身份/金额、允许轨道、确认数、链上来源、交易号、链上应付和实付数量必须完整；应付/实付数量都缺失时不能跳过比较，转入 `paid_amount_missing` 复核。本地冻结应付数量优先于网关后续返回值，法币相等不能豁免链上少付。

默认滚动限额：

| 范围 | 默认上限 |
| --- | --- |
| 用户 24 小时充值 | 2,000 美元 |
| 渠道 24 小时充值 | 20,000 美元 |
| 全站 24 小时充值 | 50,000 美元 |
| 全站每小时充值 | 10,000 美元 |
| 用户 24 小时转账及 C2C 收入 | 10,000 美元 |

建单额度包含尚未结案的旧订单，不能靠跨日或创建新幂等键绕过；到账速度以实际回执观测时间重查。单用户迟到付款超过个人额度时保留订单待后续核实；渠道/全站速度超限会原子打开持久化充值熔断、保存风险事实并通知 Owner。熔断阻止新充值与自动入账，不清除真实付款证据。Owner 在「设置 → 市场 → 平台钱包 → 资金聚合限额」核实后，以版本 CAS 和登录密码确认修改策略/解除熔断（该表单已从用户的平台余额页移出）；后台继续处理原单。回执按链、资产和交易号跨商户防复用。

公开支付回调保留已有每 IP 每分钟 600 次限流。锁内仅取最多 16 个渠道的密文/版本快照；解密与验签移到锁外，最终事务重新检查密钥 revision 与 grace 状态。Owner 轮换与并发回调不能依靠旧快照扩大有效密钥窗口。

## 可选独立链上复核

默认继续要求网关认证查单；Owner 可额外开启 `chainVerificationRequired`。开启前由部署管理员设置 `MARKET_CHAIN_ANCHORS_FILE`，并提供每个渠道/资产的独立 RPC、网络、收款地址和合约/资产锚点。缺配置、错误网络、错误资产、RPC 不可用、确认不足或数量不符一律保留复核，不能回退为网关单方证明。

```json
{
  "rails": [
    {
      "channelId": "REPLACE_WITH_CHANNEL_ID",
      "tokenId": "ethereum-usdc",
      "rpcUrl": "https://independent-node.example.invalid",
      "recipient": "REPLACE_WITH_TREASURY_ADDRESS",
      "assetId": "REPLACE_WITH_INDEPENDENTLY_VERIFIED_USDC_CONTRACT",
      "decimals": 6,
      "networkId": "0x1"
    }
  ]
}
```

这是结构示例，不能直接用于生产。锚点不得从待核实网关响应自动复制。配置文件与 RPC 凭据由部署权限保护，验证记录只存 RPC host，不保存 URL 密钥。RPC 要求 HTTPS 或本地节点，禁重定向，单次查询 8 秒、总体 25 秒、单响应最多 2 MiB，所有网络 I/O 在数据库锁外。

- EVM：chain ID、成功 receipt、canonical block、确认数，以及指定合约对指定地址的净 Transfer 数量。原生币要求外部账户收款地址、非自转账和直接 value 转入。
- Tron：genesis block ID、solidified 成功交易、确认数、TRX 直接转入或指定 TRC20 合约净转入。Tron 地址可为 Base58Check 或 `41…` 十六进制。
- Solana：genesis hash、finalized 成功交易、slot 确认差、指定 mint/owner 的原子余额净增量，包含关闭的旧 token account。
- Aptos：chain ID、成功 user transaction、指定 CoinStore 的事件 GUID，或 `depositStore` 在该 ledger version 的 owner/metadata；扣除同笔交易的对应取出事件。

USD 稳定币按 1 美元计价，此假设不保证稳定币不会脱锚。非稳定币还必须独立配置保守的 `usdMicrosPerToken` 和未来时间 `priceValidUntil`，过期暂停入账；运营方须定期更新独立价格。链上观察数量按该价格必须覆盖本地法币订单。证据与锚点不可变，更新原生币估值会追加证据，不能让同一链上付款流入其他订单。RPC、锚点、稳定币价格和部署权限仍是信任边界，并非无需信任的轻客户端。

## C2C 反占款与独立恢复

| 买家等级 | 条件 | 单笔/进行中上限 |
| --- | --- | --- |
| new | 未满足更高等级 | 100 美元 / 1 笔 |
| regular | 账龄至少 7 天、累计成交至少 500 美元、3 笔、2 个不同卖家 | 500 美元 / 2 笔 |
| established | 账龄至少 30 天、累计成交至少 2,000 美元、10 笔、3 个不同卖家 | 5,000 美元 / 2 笔 |

仍受平台/卖家范围、余额、共享转出限额和源 IP 进行中限额约束。每卖家同时接受的新买家最多 3 单，金额上限为卖家已入账余额四分之一，并限制在 100–1,000 美元之间。每单有全长唯一 `C2C-…` 付款备注码。

24 小时内三次未付款取消/超时后冷却 24 小时；经独立裁定的虚假付款声明暂停新购入。原有订单、付款证据、补充说明和申诉不受新购入限制影响。

卖家可对 paid/disputed 单报告“未收到款”，进入争议和 24 小时补证流程。超时只提醒并升级独立裁决，不能把沉默自动认定为未付款并解冻。主动报告该问题的卖家不因这单满 60 分钟自动暂停新单。买家逾期仍可补交证据。

独立 Owner/授权处理人可依据实际证据选择正常归还或 `verified_false_payment`。当事人不得裁决自己的订单。误判解除使用原 decisionId 和外部证据追加 `c2c_false_payment_cleared`，不删除原风险记录、不改终态账本，不解除其他独立限制。管理端订单详情提供此恢复入口，并同样要求登录密码确认。

## 新收款人待到账与撤回

报价冻结金额、收款人邮箱、脱敏账户提示，并返回 `delivery: instant|delayed` 与 `delayHours`，前端在确认前展示。收款账户、余额、服务资金承诺及限额在提交事务内重查。

- 已知收款人：即时过账，与原流程相同。≥ 500 美元时，业务事务内再次检查密码冷静期，防止分类之后对方恰好变成已知收款人而绕过。
- 新收款人：在同一事务内记录 `market_financial_authorizations`、对转出人余额建立 `wallet_transfer_pending` 冻结，并写入 `market_wallet_pending_transfers`（`release_at` = 确认时间 + 24 小时），只通知转出人。收款人看不到待到账转账，因此无法被当作“已付款”凭证。
- 每日转出额度与收款人 24 小时收入额度都把待到账金额计入（视图 `market_wallet_transfer_commitments`）。
- 到期由“钱包履约”后台任务（5 秒周期，单条独立事务）释放冻结并以确认时间、原授权和原幂等键生成普通 `market_wallet_transfers` 记录，因此 `market_verified_wallet_transfer_evidence` 不变。任一方处于财务核对冻结时延后；收款人不可用时退回转出人。
- 撤回：`POST /v1/market-wallet/transfers/pending/:id/cancel`，只允许转出人、只在到账前；钱只回到本人，不需要密码。`GET /v1/market-wallet/transfers/pending` 列出转出人的待到账转账。
- 对账：`wallet_pending_transfer_mismatch` 检查待到账记录与冻结一一对应、已到账记录与正式转账一致、已结束记录不残留有效冻结；异常冻结双方用户范围。

旧的“收款人同意退回”流程退役：`POST /transfers/:id/return` 与 Owner 协助申请返回 410。迁移前已提交、仍待处理的申请，收款人仍可在“待处理”中同意（需要密码，立即生效）或拒绝，`GET /v1/market-wallet/transfer-returns/awaiting` 列出这些申请。历史 consent、补偿 journal 与对账检查保留。

## 对账与部署

对账使用持久化 keyset cursor 和环回扫描。连续存在的首批异常不能饿死后续账本、充值、转账、退回、合约或收益证据。新增签名证明校验和退回 consent/journal 检查，异常只冻结关联用户；孤立账本、封存异常仍全局阻断。记录被使用的证明不会因 Session 清理或 30 天无用证明回收而丢失。

部署前备份数据库和凭据配置，先部署识别 114–116 的后端，再发布匹配前端。迁移 114 会一次性撤销现有登录会话、提升 auth_version 并清除 refresh rotation grace，所有用户需重新登录；旧域 Cookie 在后续认证响应中清除。迁移 121 之后，Owner 高危操作使用登录密码确认；新设置的密码须等待 24 小时。不可删除迁移记录或回滚到不认识新模式的二进制。

本地 mock/RPC 回归不能代替真实商户验收。本次实现不自动打开生产支付、不发送真实付款、不修改远程部署。生产开启独立验证前应使用运营方控制的收款地址，对每个实际开放轨道验证真实交易、金额、确认数、错链/错资产、重复付款、迟到付款、RPC 故障、熔断与恢复；验收通过的轨道逐个开放。
