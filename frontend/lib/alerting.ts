const ALERT_CHANNEL_LABELS: Record<string, string> = {
  telegram: "Telegram",
};

export function alertChannelLabel(channel: string) {
  return ALERT_CHANNEL_LABELS[channel] || channel;
}
