const ALERT_CHANNEL_LABELS: Record<string, string> = {
  telegram: "Telegram",
  bark: "Bark",
};

export function alertChannelLabel(channel: string) {
  return ALERT_CHANNEL_LABELS[channel] || channel;
}
