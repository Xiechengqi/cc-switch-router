/**
 * Seller-side seat termination rules and buyer-facing seat terms on cards.
 * Owned by the owner seat editor and the market card.
 */
export const shareSeatTermsMessagesEn = {
  "shareSeatTerms.title": "Ending early",
  "shareSeatTerms.allowed": "Buyers may end any time",
  "shareSeatTerms.forbidden": "No early ending",
  "shareSeatTerms.forbiddenHelp": "Buyers can't end a paid period early; they turn off renewal and the rental ends with the period. If the service is broken, they can still report it: all unused rent is held until you respond or the platform decides.",
  "shareSeatTerms.rate": "Compensation for ending early",
  "shareSeatTerms.platformDefault": "Platform default",
  "shareSeatTerms.platformDefaultRate": "Platform default {rate}",
  "shareSeatTerms.rateHelp": "When a buyer ends early, you keep at most this share of one full period's rent, 0–{max}%. Leave empty to use the platform default. Trials never pay compensation.",
  "shareSeatTerms.rateInvalid": "Enter a compensation rate from 0 to 50%, with at most two decimals, or leave it empty.",
  "shareSeatTerms.newRentalsOnly": "Applies to new rentals only. Current rentals keep the terms they were rented with.",
  "shareSeatTerms.unlimitedQuota": "Unlimited quota",
  "shareSeatTerms.unlimitedParallel": "Unlimited parallel",
  "shareSeatTerms.parallel": "{value} parallel",
  "shareSeatTerms.trial": "{hours}h trial",
  "shareSeatTerms.noEarlyEndTag": "No early end",
  "shareSeatTerms.noEarlyEndHint": "This seat can't be ended early. You can turn off renewal so it ends with the paid period.",
  "shareSeatTerms.performanceLabel": "P50 first token / speed",
};

export const shareSeatTermsMessagesZh: Record<keyof typeof shareSeatTermsMessagesEn, string> = {
  "shareSeatTerms.title": "退租规则",
  "shareSeatTerms.allowed": "允许随时退租",
  "shareSeatTerms.forbidden": "不允许中途退租",
  "shareSeatTerms.forbiddenHelp": "买家不能中途结束付费周期，只能关闭续费、到期结束。如服务异常，买家仍可报告服务问题：全部未用租金将暂扣，待你回应或平台裁决。",
  "shareSeatTerms.rate": "退租补偿比例",
  "shareSeatTerms.platformDefault": "平台默认",
  "shareSeatTerms.platformDefaultRate": "平台默认 {rate}",
  "shareSeatTerms.rateHelp": "买家中途退租时，你最多获得完整周期租金的该比例作为补偿，范围 0–{max}%。留空则使用平台默认。试用期退租不产生补偿。",
  "shareSeatTerms.rateInvalid": "补偿比例需为 0–50%，最多两位小数；或留空使用平台默认。",
  "shareSeatTerms.newRentalsOnly": "仅对之后的新租用生效；已租用的车位保持租用时的规则。",
  "shareSeatTerms.unlimitedQuota": "不限额度",
  "shareSeatTerms.unlimitedParallel": "不限并发",
  "shareSeatTerms.parallel": "{value} 并发",
  "shareSeatTerms.trial": "试用 {hours}h",
  "shareSeatTerms.noEarlyEndTag": "不可中途退租",
  "shareSeatTerms.noEarlyEndHint": "此车位不支持中途退租，可关闭续费后到期结束。",
  "shareSeatTerms.performanceLabel": "P50 首字 / 速度",
};
