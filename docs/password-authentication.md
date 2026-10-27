# Router 密码登录与本机密码管理

## 用户流程

Router 默认使用邮箱验证码登录，也可以选择密码登录。密码是可选项，不替代邮箱所有权验证；未注册或尚未设置密码的邮箱都会得到同一条提示：

> 当前用户不能使用密码登录，请使用验证码登录后，再设置登录密码。

未设置密码的用户，每次真正完成验证码登录后会自动打开一次设置弹窗，可以「稍后设置」。刷新页面、切换页面、后台读取 Session 或轮换 Token 不会再次弹出。设置入口始终在右上角用户菜单中；设置后入口变成「修改登录密码」。登录回跳会等待已打开的登录／密码弹窗完成或关闭。

设置规则：

- 使用 6–128 个 Unicode 字符，不允许控制字符或全空白密码，最多 512 个 UTF-8 字节；不强制大小写、数字、符号组合。网页、API 和 CLI 使用相同规则，CLI 交互输入前显示长度要求。
- 保留首尾空格，支持长口令、粘贴和密码管理器。新密码需输入两次，可切换显示／隐藏。
- 任何有效登录会话都可以设置或修改密码，只需输入两次新密码，不需要原密码或二次邮箱验证。仍需正常登录，API Token、Cookie 回退和开发模式不能代替改密接口的显式 Session Bearer。
- 未登录且忘记密码时，使用邮箱验证码登录，再设置新密码。Session 刷新保留原始登录事件，不冒充一次新的验证码登录。
- 改密会立即撤销此前所有浏览器 access / refresh token，包括 refresh 的 30 秒并发宽限。网页改密只为当前设备签发替换会话；CLI 改密不签发会话。
- 不改变角色、账号禁用状态、API Token、Share 授权、租用或钱包资金。密码登录不会创建用户、验证邮箱或解除禁用。

密码设置成功只排队一封英文确认邮件，**邮件正文包含新密码明文**，网页弹窗和 CLI 均明确提醒。邮件发送失败不撤回已经提交的改密；密码登录和密码输错不会额外发邮件。

明文邮件是运营者明确选择的弱安全方案：邮箱、邮件服务商和邮件备份都可能保留可直接登录的密码，队列加密不能消除这项风险。用户应保护邮箱，不转发邮件，在密码管理器中保存密码后删除邮件。不建议复用其他网站的密码。

Router 凭据表仍只保存 Argon2id hash。为支持邮件失败重试，确认邮件内容使用独立的 XChaCha20-Poly1305 密钥加密后临时排队，绑定具体改密事件和收件邮箱；发送成功、永久失败、过期或被新密码取代后清空队列载荷。日志、审计和 CLI 输出不记录密码。Router 停止期间无法执行过期清理，重新启动后会继续；数据库历史备份和收件端邮件不在此清理范围内。

独立密钥位于 `<CC_SWITCH_ROUTER_DATA_DIR>/password-notice-key`，首次需要时原子创建，Unix 权限 `0600`。必须连同数据库妥善保护和备份；存在待发送密文时密钥丢失不会静默重新生成。无法安全排队时整笔改密回滚，避免已改密却无法创建所要求的通知。

## 本机 CLI：所有已有用户均可设置／替换密码

以下命令只供 Router 部署管理员使用，不是 Router HTTP 管理 API。Router owner、Provider 和普通用户使用同一套命令；不要求原密码，也不依赖邮件渠道可用。

### 在线管理（默认，推荐）

在 Router 所在主机上，以 root 或 Router 服务进程的系统用户执行，使用部署时的同一配置文件：

```bash
cc-switch-router user password status --email user@example.com --env-file /path/to/router.env
cc-switch-router user password set --email user@example.com --env-file /path/to/router.env
```

`set` 会先展示目标 Router、数据库、用户和已有密码状态，要求确认，再用无回显方式输入两次新密码。不存在的用户直接报错，不会顺便注册账号。

自动化可通过标准输入传递密码，必须同时使用 `--yes`：

```bash
secret-tool lookup service router-login user user@example.com |
  cc-switch-router user password set --email user@example.com \
    --env-file /path/to/router.env --password-stdin --yes
```

示例中的 `secret-tool` 仅表示从受控密码源读取，可替换为自己的 Secret Manager。不要把明文密码放进命令行、shell 历史、环境变量或日志。CLI 不支持 `--password 明文`。stdin 最多读取 1024 字节，仅剥除末尾一个 LF 或 CRLF，不 trim 密码空格。

命令在发送设置请求前打印操作 UUID。若超时或连接中断，不要立即再次重置，更不要自动改用离线写入；先查询操作结果：

```bash
cc-switch-router user password result --operation-id OPERATION_UUID --env-file /path/to/router.env
```

查到结果代表该操作已提交，输出的是该用户当前凭据状态，不包含密码或 hash。其他重置可能已使当前版本高于原操作版本；原操作版本记录在 `auth_password_events`。查不到时再运行 `status`，结合审计后决定是否重试。

在线通道固定为 `<CC_SWITCH_ROUTER_DATA_DIR>/admin/user-password.sock`，目录 `0700`、socket `0600`；双方检查系统 UID，服务端仅接受 root 或自身 UID。连接数、请求字节数和处理时间均有限制。不会开放 TCP 管理端口。拒绝不安全的目录权限、符号链接或同名普通文件，不会覆盖它们。

### 离线救援（必须显式选择）

Router 已停止时：

```bash
cc-switch-router user password set --email user@example.com \
  --env-file /path/to/router.env --offline
```

离线模式仍要求 root 或数据目录所属系统用户，且数据目录和数据库必须已经存在、不能是符号链接。取得与 Router 启动相同的独占进程锁后才操作数据库；Router 正在运行时直接拒绝。不会自动初始化空库、运行迁移或创建用户。

首次升级到支持密码的版本，先正常启动 Router 一次以安装 migration 90，再使用离线命令。若数据目录／数据库在配置中使用相对路径，执行命令时必须与服务使用同一工作目录；生产建议使用绝对路径。以不同系统用户执行时，也必须明确选中部署配置，不能误用该用户默认数据目录。

支持项目已有 local / Turso 数据库模式；Turso 仍需要远端可用，不允许断网写入，也不能用其他实例绕过单个可写 Router 的部署约束。没有邮件配置时变更照常提交，通知留在 outbox，启动且邮件恢复后在投递窗口内尝试发送。

## 安全、事务和限流

- Argon2id v19：64 MiB、3 次迭代、1 lane、每次独立随机盐。只存 PHC hash；最多同时执行 2 个计算任务，阻塞任务取消前也持有内存预算。
- 未配置密码和未知账号均执行同成本 dummy 验证，不提供公开密码状态查询接口。已配置账号输错密码／禁用／未验证邮箱返回统一错误。
- 登录使用 auth device Ed25519 签名，action 为 `auth_password_login`，绑定完整邮箱、密码、时间和 nonce。nonce 独立于验证码请求记录，不能重放。只允许可信浏览器 Origin，非浏览器签名客户端可省略 Origin。
- 修改密码仅接受真实 Session 的显式 Bearer，不能由 Cookie、开发模式或 API Token 代替。前后端均已移除原密码字段和「最近 10 分钟邮箱验证」门槛；真实会话、过期时间、账户状态和版本仍由数据库校验，哈希前后各检查一次。
- 15 分钟持久化尝试预算：邮箱+来源 IP 10 次、同一邮箱跨 IP 100 次、来源 IP 60 次、设备 20 次、Router 全局 2000 次。密码登录与网页改密均在 Argon2 前预留，失败后也计数，重启不清零；OTP 通道不消耗这些密码预算。并发计算繁忙返回可重试 `503`。
- 网页和 CLI 共用同一 version-CAS 事务：升级 `users.auth_version`、替换 hash、撤销全部旧 Session、写一条审计和一条通知；任一失败整体回滚。
- Session 解析和 refresh 在写事务中向权威数据库检查版本。Turso 不能用尚未同步的本地副本继续接受已经撤销的登录会话。
- 网页密码请求固定到发起时的登录上下文；普通 token 轮换可以继续，但其他标签页切换账号、重新登录或主动退出后，不会以新会话重试旧改密请求，也不会用旧响应覆盖新登录。
- 密码变更邮件使用加密冻结 envelope、稳定 Resend 幂等键和 2 分钟 lease；每次临时故障至少延后 1 小时，最多 6 次，最迟 12 小时停止并清除载荷。未配置邮件也会执行过期清理。再次改密会清理此前未完成的通知，防止继续重试已失效密码的邮件；已经开始的外部投递无法撤回。成功只记录一次 `email_send_logs`；常规登录不产生安全通知，队列故障不会永久终止 worker。

## 验收与运维

```bash
cargo test --locked password_auth::tests
cargo test --locked user_cli::tests
cargo test --locked session_
cargo test --locked auth_code_verification_
cargo test --locked schema::tests
npm --prefix frontend run test:auth-password
npm --prefix frontend run test:auth-password-browser
```

浏览器测试使用本地静态导出、mock API 和真实设备签名，不截屏、不发邮件、不改生产用户。密码、安全通知和 CLI 测试均已加入 CI。发布后无需强制已有用户立即改密：原 Session 与 API Token 保持原行为，只有用户主动设置／重置密码时才撤销其旧 Session。
