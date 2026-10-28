export const clientTrialMessagesEn = {
  "clientTrial.allowance": "Trial allowance for this Host",
  "clientTrial.consumed": "Used in this trial round",
  "clientTrial.remaining": "Available for this rental",
  "clientTrial.exhausted": "Your trial allowance for this Host is used up. Successful activation will charge the first period: {price}.",
  "clientTrial.disabled": "This Host does not offer a free trial.",
  "clientTrial.rules": "Trial-only rentals share the same allowance on this Host. A successful rent payment restores one full allowance for your next rental; renewal never stacks allowances.",
  "clientTrial.restored": "Next-rental trial eligibility restored. Current allowance: {duration}, subject to the Host's terms when you rent again. This does not interrupt current paid billing.",
  "clientTrial.restoredDisabled": "Next-rental trial eligibility restored, but this Host currently has trials disabled. Current paid billing is unchanged.",
  "clientTrial.releasePaid": "Ending this rental, returning unused rent to your wallet, or reducing/waiving compensation does not cancel your restored next-rental trial eligibility. Re-renting still requires verified Host cleanup.",
  "clientTrial.releaseUnpaid": "No rent has been paid: ending this trial does not reset its allowance. Unused trial time remains available when you rent this Host again.",
  "clientTrial.settings": "Client trials are shared across rentals by the same account on the same Provider's Host. A successful rent debit restores the next rental's allowance, regardless of later termination compensation or wallet credit returns. No daily reset or stacked rewards. Share trials remain one-time.",
} as const;

export const clientTrialMessagesZh: Record<keyof typeof clientTrialMessagesEn, string> = {
  "clientTrial.allowance": "此 Host 本轮试用上限",
  "clientTrial.consumed": "本轮已使用",
  "clientTrial.remaining": "本次可试用",
  "clientTrial.exhausted": "你在此 Host 的本轮试用额度已用尽。开通成功后将直接支付首期租金：{price}。",
  "clientTrial.disabled": "此 Host 不提供免费试用。",
  "clientTrial.rules": "同一 Host 的多次纯试用租用共用一份额度。正式租金扣款成功后，恢复下一次租用的一份完整额度；续费不叠加多份额度。",
  "clientTrial.restored": "下次租用的试用资格已恢复。当前上限为 {duration}，实际以再次租用时 Host 的条款为准；不影响当前正常付费计费。",
  "clientTrial.restoredDisabled": "下次租用的试用资格已恢复，但此 Host 当前关闭了试用；当前付费计费不变。",
  "clientTrial.releasePaid": "立即退租、剩余租金退回平台余额或补偿减免，都不会取消已恢复的下次试用资格。Host 仍须确认清理成功后才能再次租用。",
  "clientTrial.releaseUnpaid": "本次尚未支付租金：结束试用不会重置额度。再次租用此 Host 时，可继续使用本轮未消耗的试用时间。",
  "clientTrial.settings": "同账号在同一 Provider 的同一 Host 上多次租用共用试用额度。正式租金扣款成功后恢复下次租用的额度，不以退租补偿或租金是否退回平台余额为条件。不按天恢复，不叠加多份奖励。Share 仍保持一次性试用。",
};
