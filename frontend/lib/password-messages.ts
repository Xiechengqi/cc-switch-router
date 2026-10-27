export const passwordMessagesEn = {
  "auth.password.emailTab": "Email code",
  "auth.password.passwordTab": "Password",
  "auth.password.label": "Password",
  "auth.password.signIn": "Sign in",
  "auth.password.show": "Show password",
  "auth.password.hide": "Hide password",
  "auth.password.forgot": "Forgot password?",
  "auth.password.useEmail": "Use an email code",
  "auth.password.setTitle": "Set login password",
  "auth.password.changeTitle": "Change login password",
  "auth.password.setupHint":
    "Use a password to sign in without waiting for email. Email code login remains available.",
  "auth.password.changeHint":
    "Your other login sessions will be signed out. API Tokens and rented services are unchanged.",
  "auth.password.current": "Current password",
  "auth.password.new": "New password",
  "auth.password.confirm": "Confirm new password",
  "auth.password.later": "Not now",
  "auth.password.save": "Save password",
  "auth.password.saved": "Password saved",
  "auth.password.savedHint":
    "You can now sign in with your email and password.",
  "auth.password.mismatch": "Passwords do not match.",
  "auth.password.policy":
    "Use 12–128 characters without control characters. Spaces and passphrases are welcome.",
  "auth.password.unavailable":
    "This account cannot use password login. Sign in with an email code, then set a login password.",
  "auth.password.invalid": "Email or password is incorrect.",
  "auth.password.currentInvalid": "Current password is incorrect.",
  "auth.password.emailRequired":
    "Sign in again with an email code before setting or resetting a password.",
  "auth.password.changedElsewhere":
    "Password changed elsewhere. Reload your session and retry.",
  "auth.password.busy":
    "Password authentication is busy. Please retry shortly.",
  "auth.password.rateLimited":
    "Too many password attempts. Try again later or use an email code.",
  "auth.password.sessionChanged":
    "Your sign-in session changed. Sign in again.",
} as const;

export const passwordMessagesZh: Record<
  keyof typeof passwordMessagesEn,
  string
> = {
  "auth.password.emailTab": "验证码登录",
  "auth.password.passwordTab": "密码登录",
  "auth.password.label": "密码",
  "auth.password.signIn": "登录",
  "auth.password.show": "显示密码",
  "auth.password.hide": "隐藏密码",
  "auth.password.forgot": "忘记密码？",
  "auth.password.useEmail": "使用验证码登录",
  "auth.password.setTitle": "设置登录密码",
  "auth.password.changeTitle": "修改登录密码",
  "auth.password.setupHint": "设置后可用密码快捷登录，仍可使用邮箱验证码登录。",
  "auth.password.changeHint":
    "其他登录会话将退出，API Token 和已租服务不受影响。",
  "auth.password.current": "当前密码",
  "auth.password.new": "新密码",
  "auth.password.confirm": "确认新密码",
  "auth.password.later": "稍后设置",
  "auth.password.save": "保存密码",
  "auth.password.saved": "密码已保存",
  "auth.password.savedHint": "现在可以使用邮箱和密码登录。",
  "auth.password.mismatch": "两次输入的密码不一致。",
  "auth.password.policy":
    "使用 12–128 个字符，不含控制字符；支持空格、长口令及密码管理器。",
  "auth.password.unavailable":
    "当前用户不能使用密码登录，请使用验证码登录后，再设置登录密码。",
  "auth.password.invalid": "邮箱或密码不正确。",
  "auth.password.currentInvalid": "当前密码不正确。",
  "auth.password.emailRequired":
    "请先重新使用邮箱验证码登录，再设置或重置密码。",
  "auth.password.changedElsewhere":
    "密码已在其他位置修改，请刷新登录状态后重试。",
  "auth.password.busy": "密码验证繁忙，请稍后重试。",
  "auth.password.rateLimited": "密码尝试次数过多，请稍后重试或使用验证码登录。",
  "auth.password.sessionChanged": "登录会话已变更，请重新登录。",
};
