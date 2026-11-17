# 平台钱包安全与异常恢复

本文为迁移 114–116 后的当前契约，覆盖官方充值、C2C 和邮箱转账。早期文档中的“仅凭登录会话即可操作资金”已被替代。历史账本、OTP 证据和已成交订单不改写。

本轮发现的问题、修正证据与测试范围见 [整体 review](market-wallet-security-review.md)。

## 身份边界

- 所有 Share、Client Web 和 Gateway 隧道请求剥离 Cookie；Share/Gateway 同时剥离调用方 Authorization、API Key。上游 Set-Cookie 不回传浏览器。
- HTTPS 浏览器凭据为 `__Host-cc_switch_router_session`，不带 Domain，具有 Secure、HttpOnly、Path=/、SameSite=Lax 属性。仅 localhost 开发环境使用不带 Secure 的 `cc_switch_router_session`。
- Cookie 是独立随机 `browser_…` 凭据，数据库仅存其哈希，不能作为 Bearer。它随原登录会话撤销、轮换和过期失效。
- `/v1/auth/*`、`/v1/admin/*`、`/v1/market-wallet[/…]`、`/v1/market-provider-finance/*` 只在 Router 主 Host 提供。Share Host 上返回 404。
- 财务写入必须同时持有真实 Session Bearer 和本次登录设备的签名。Cookie、API Key、开发模式身份、单独新注册的设备不能代替。提供 Origin 时必须精确匹配部署配置；浏览器跨站请求被拒绝。
- 最终 IMMEDIATE 资金事务重新检查 Session 撤销、访问/刷新有效期、用户状态、auth_version、设备撤销和请求证明。网络查询前的预检不能代替最终检查。

设备 Ed25519 私钥与登录令牌仍在现有浏览器本地存储模型中。设备签名证明请求持有设备密钥，并防止单独盗用 Bearer；它不是独立 MFA，不能抵御 Router 主站 XSS、恶意浏览器扩展或完整设备失陷。

## 请求签名与 Passkey

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

新收款人、500 美元及以上转账、500 美元及以上 C2C 放行、收款人同意退回，以及 Owner/管理端财务写入要求 WebAuthn Passkey 用户验证。服务端以 428 返回 challenge 和 options；前端完成验证后，以 `X-Router-Stepup`、原始正文和原业务幂等键、全新 nonce 重发。Passkey grant 绑定用户、Session、请求正文、路径及幂等键，在业务事务中消费。事务失败时消费一并回滚，其他请求不能复用已消费 grant。已提交转账/退回的业务重放仍返回原结果，不再次转款。

Passkey 仅作用于 Router RP ID 和精确 origin，验证要求 UV。challenge 在服务端保存、5 分钟到期、一次完成；认证计数器与当前数据库值在同一事务比较，避免并行旧 challenge 回退计数。支持计数器始终为零的同步 Passkey。

首次登记要求最近 15 分钟内真正通过邮箱验证码登录；密码登录/刷新不延长邮箱证明。新 Passkey 有 24 小时冷静期，最多 8 个未撤销凭据；追加凭据必须先用已有可用 Passkey 确认。登记重试会替换未完成 ceremony；恢复许可在成功登记时消费，失败登记不会耗掉许可。

丢失全部 Passkey 后不能用现有钱包 HTTP 会话自行清空凭据。部署管理员核实身份后，通过已有 UID 限制 Unix 管理 socket 或严格离线锁执行：

```bash
cc-switch-router user finance-passkey recover --email USER_EMAIL --session-id SESSION_REFERENCE --operation-id UUID --env-file /path/to/.env
cc-switch-router user finance-passkey approve --email USER_EMAIL --key-id PASSKEY_ID --operation-id UUID --env-file /path/to/.env
```

recover 必须指向该用户最近 15 分钟邮箱登录的真实有效 Session，撤销所有旧 Passkey、其他 Session 和旧 ceremony，并给目标 Session 15 分钟重新登记许可；新凭据仍受冷静期约束。approve 仅提前启用指定未撤销凭据，不能恢复被撤销的密钥。operation UUID 幂等，保留本地操作者及安全审计。

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

建单额度包含尚未结案的旧订单，不能靠跨日或创建新幂等键绕过；到账速度以实际回执观测时间重查。单用户迟到付款超过个人额度时保留订单待后续核实；渠道/全站速度超限会原子打开持久化充值熔断、保存风险事实并通知 Owner。熔断阻止新充值与自动入账，不清除真实付款证据。Owner 在钱包安全面板核实后，以版本 CAS 和 Passkey 明确修改策略/解除熔断；后台继续处理原单。回执按链、资产和交易号跨商户防复用。

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

独立 Owner/授权处理人可依据实际证据选择正常归还或 `verified_false_payment`。当事人不得裁决自己的订单。误判解除使用原 decisionId 和外部证据追加 `c2c_false_payment_cleared`，不删除原风险记录、不改终态账本，不解除其他独立限制。管理端订单详情提供此恢复入口，并同样要求 Passkey。

## 转账纠错

报价冻结金额、收款人邮箱、脱敏账户提示和新收款人标识；前端明确展示后确认。收款账户、余额、服务资金承诺及限额在提交事务内重查。

转出人可对原转账申请全额退回；Owner 只能协助发起申请。只有原收款人可通过 Passkey 同意或拒绝，管理员不能强制扣款。批准在同一事务内检查可用额及应优先履行的服务承诺，保存不可变 consent，创建双边补偿 journal、更新两端余额、结案并通知。余额已花掉或被冻结时不能产生负余额。原转账始终保留，一笔转账仅有一个退回申请与最多一个补偿 journal。

## 对账与部署

对账使用持久化 keyset cursor 和环回扫描。连续存在的首批异常不能饿死后续账本、充值、转账、退回、合约或收益证据。新增签名证明校验和退回 consent/journal 检查，异常只冻结关联用户；孤立账本、封存异常仍全局阻断。记录被使用的证明不会因 Session 清理或 30 天无用证明回收而丢失。

部署前备份数据库和凭据配置，先部署识别 114–116 的后端，再发布匹配前端。迁移 114 会一次性撤销现有登录会话、提升 auth_version 并清除 refresh rotation grace，所有用户需重新登录；旧域 Cookie 在后续认证响应中清除。初次 Owner 高危操作前须登记并等冷静期，或经本地管理员核实后批准指定 Passkey。不可删除迁移记录或回滚到不认识新模式的二进制。

本地 mock/RPC 回归不能代替真实商户验收。本次实现不自动打开生产支付、不发送真实付款、不修改远程部署。生产开启独立验证前应使用运营方控制的收款地址，对每个实际开放轨道验证真实交易、金额、确认数、错链/错资产、重复付款、迟到付款、RPC 故障、熔断与恢复；验收通过的轨道逐个开放。
