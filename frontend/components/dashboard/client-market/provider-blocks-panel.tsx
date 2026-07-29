"use client";

import * as React from "react";
import { toast } from "@heroui/react";
import { UserBlacklistPanel } from "@/components/common/user-blacklist-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  createClientMarketProviderBlock,
  getClientMarketProviderBlocks,
  liftClientMarketProviderBlock,
} from "@/lib/api";
import type { ClientMarketProviderBlock } from "@/lib/types";

function blockReasonLabel(reason: string, t: ReturnType<typeof useLocaleText>["t"]) {
  if (reason === "payment_not_received") return t("account.blockReason.paymentNotReceived");
  if (reason === "manual") return t("clientMarket.blockReason.manual");
  return reason.replaceAll("_", " ");
}

/** Provider-owned user blacklist — lives on Client Market, not Account payments. */
export function ProviderBlocksPanel({
  enabled,
  hosting = false,
}: {
  enabled: boolean;
  hosting?: boolean;
}) {
  const { t } = useLocaleText();
  const [blocks, setBlocks] = React.useState<ClientMarketProviderBlock[]>([]);
  const [loading, setLoading] = React.useState(false);

  const reload = React.useCallback(async () => {
    if (!enabled) {
      setBlocks([]);
      return;
    }
    setLoading(true);
    try {
      setBlocks(await getClientMarketProviderBlocks());
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }, [enabled]);

  React.useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <UserBlacklistPanel
      enabled={enabled}
      hosting={hosting}
      loading={loading}
      entries={blocks.map((block) => ({
        id: block.clientUserId,
        email: block.clientOwnerEmail,
        reason: block.reason,
        createdAt: block.createdAt,
      }))}
      hint={t("clientMarket.blockedHint")}
      empty={t("clientMarket.noneBlocked")}
      reasonLabel={(reason) => blockReasonLabel(reason, t)}
      onAdd={async (emails) => {
        const created: ClientMarketProviderBlock[] = [];
        for (const email of emails) {
          created.push(await createClientMarketProviderBlock({ email, reason: "manual" }));
        }
        setBlocks((current) => {
          let next = current;
          for (const item of created) {
            next = [item, ...next.filter((entry) => entry.clientUserId !== item.clientUserId)];
          }
          return next;
        });
        if (created.length === 1) {
          toast.success(t("clientMarket.blockedAddedToast", { email: created[0].clientOwnerEmail }));
        } else {
          toast.success(t("clientMarket.blockedAddedCountToast", { count: created.length }));
        }
      }}
      onLift={async (id) => {
        const target = blocks.find((block) => block.clientUserId === id);
        await liftClientMarketProviderBlock(id);
        setBlocks((current) => current.filter((item) => item.clientUserId !== id));
        if (target) toast.success(t("account.unblockedToast", { email: target.clientOwnerEmail }));
      }}
    />
  );
}
