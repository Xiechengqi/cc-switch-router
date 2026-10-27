export type MarketDelivery = {
  id: string; eventId: string; recipientId: string; channel: string; purpose: string;
  submissionStatus: string; providerStatus: string | null; providerMessageId: string | null;
  attempts: number; createdAt: string; firstAttemptAt: string | null; acceptedAt: string | null;
  providerStatusAt: string | null; providerCheckedAt: string | null; suppressionReason: string | null;
  submissionError: string | null; providerError: string | null; objectId: string; eventKind: string; targetMasked: string;
};
export type MarketNotificationDiagnostics = {
  items: MarketDelivery[]; nextCursor: string | null; webhookConfigured: boolean;
  limits: { pauseCooldownSecs: number; pauseDailyLimit: number; userHourlyLimit: number; emailDailyLimit: number; maxAgeSecs: number };
  overview: {
    events: Record<string, number>; submissions: Record<string, number>; provider: Record<string, number>; suppressions: Record<string, number>;
    pause: { eventCreatedAt: string | null; acceptedAt: string | null; pending: number };
    tracking: { lastPollAt: string | null; lastWebhookAt: string | null; pollErrorCode: string | null; pollBackoffUntil: string | null };
  };
};

/** Submission success is never labelled as delivery success. */
export function deliveryState(item: Pick<MarketDelivery, "channel" | "submissionStatus" | "providerStatus">): string {
  if (item.channel === "email" && item.submissionStatus === "sent") return item.providerStatus || "untracked";
  return item.submissionStatus === "sent" ? "accepted" : item.submissionStatus;
}
export function providerFailureCount(counts: Record<string, number>): number {
  return (counts.failed || 0) + (counts.bounced || 0) + (counts.complained || 0) + (counts.suppressed || 0);
}
export const deliveryLabels = {
  en: { accepted: "Gateway accepted", delayed: "Delivery delayed", delivered: "Delivered to mail server", failed: "Failed", bounced: "Bounced", complained: "Spam complaint", suppressed: "Suppressed", queued: "Queued", claimed: "In flight", uncertain: "Acceptance uncertain", blocked_config: "Not configured", untracked: "Delivery not tracked" },
  "zh-CN": { accepted: "网关已接收", delayed: "投递延迟", delivered: "已交付收件服务器", failed: "失败", bounced: "退信", complained: "垃圾邮件投诉", suppressed: "已抑制", queued: "排队中", claimed: "提交中", uncertain: "接受结果不明", blocked_config: "未配置渠道", untracked: "未追踪投递" },
} as const;
