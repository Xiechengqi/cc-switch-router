"use client";

import * as React from "react";
import { Button, toast } from "@heroui/react";
import { Loader2, Plus, ShieldBan } from "lucide-react";
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
  /** True when the viewer currently hosts at least one Client Market host. */
  hosting?: boolean;
}) {
  const { locale, t } = useLocaleText();
  const [blocks, setBlocks] = React.useState<ClientMarketProviderBlock[]>([]);
  const [loading, setLoading] = React.useState(false);
  const [email, setEmail] = React.useState("");
  const [adding, setAdding] = React.useState(false);
  const [liftingBlock, setLiftingBlock] = React.useState("");

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

  const add = async () => {
    const value = email.trim();
    if (!value || adding) return;
    setAdding(true);
    try {
      const created = await createClientMarketProviderBlock({ email: value, reason: "manual" });
      setBlocks((current) => {
        const without = current.filter((item) => item.clientUserId !== created.clientUserId);
        return [created, ...without];
      });
      setEmail("");
      toast.success(t("clientMarket.blockedAddedToast", { email: created.clientOwnerEmail }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setAdding(false);
    }
  };

  const unblock = async (block: ClientMarketProviderBlock) => {
    setLiftingBlock(block.clientUserId);
    try {
      await liftClientMarketProviderBlock(block.clientUserId);
      setBlocks((current) => current.filter((item) => item.clientUserId !== block.clientUserId));
      toast.success(t("account.unblockedToast", { email: block.clientOwnerEmail }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setLiftingBlock("");
    }
  };

  if (!enabled) return null;
  // Only providers who host (or still have leftover blocks) see this panel.
  if (!hosting && !loading && !blocks.length) return null;

  return (
    <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-3 rounded-xl border border-border bg-card p-4 shadow-sm">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <ShieldBan className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">
              {t("clientMarket.blockedOwners")}
              {blocks.length ? ` · ${blocks.length}` : ""}
            </h2>
          </div>
          <p className="mt-0.5 text-xs text-muted-foreground">{t("clientMarket.blockedHint")}</p>
        </div>
      </div>

      <form
        className="flex min-w-0 flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void add();
        }}
      >
        <input
          type="email"
          value={email}
          onChange={(event) => setEmail(event.target.value)}
          placeholder={t("clientMarket.blockEmailPlaceholder")}
          className="h-9 min-w-[16rem] flex-1 rounded-md border bg-white px-3 text-sm outline-none focus:ring-2 focus:ring-primary/20"
          disabled={adding}
        />
        <Button type="submit" size="sm" variant="outline" isDisabled={adding || !email.trim()}>
          {adding ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
          {t("clientMarket.blockAdd")}
        </Button>
      </form>

      {loading && !blocks.length ? (
        <div className="flex items-center gap-2 py-2 text-xs text-muted-foreground">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          {t("common.loading")}
        </div>
      ) : null}

      {!loading && !blocks.length ? (
        <p className="py-1 text-xs text-muted-foreground">{t("clientMarket.noneBlocked")}</p>
      ) : null}

      {blocks.length ? (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="min-w-full text-left text-sm">
            <thead className="bg-muted/40 text-xs text-muted-foreground">
              <tr>
                <th className="px-3 py-2 font-medium">{t("clientMarket.blockedCol.email")}</th>
                <th className="px-3 py-2 font-medium">{t("clientMarket.blockedCol.reason")}</th>
                <th className="px-3 py-2 font-medium">{t("clientMarket.blockedCol.since")}</th>
                <th className="px-3 py-2 font-medium">{t("common.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {blocks.map((block) => (
                <tr key={block.clientUserId} className="border-t border-border/80">
                  <td className="max-w-[18rem] truncate px-3 py-2 font-medium" title={block.clientOwnerEmail}>
                    {block.clientOwnerEmail}
                  </td>
                  <td className="whitespace-nowrap px-3 py-2 text-muted-foreground">
                    {blockReasonLabel(block.reason, t)}
                  </td>
                  <td className="whitespace-nowrap px-3 py-2 text-muted-foreground">
                    {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(
                      new Date(block.createdAt),
                    )}
                  </td>
                  <td className="px-3 py-2">
                    <Button
                      size="sm"
                      variant="outline"
                      isDisabled={!!liftingBlock}
                      onClick={() => void unblock(block)}
                    >
                      {liftingBlock === block.clientUserId ? (
                        <Loader2 className="h-4 w-4 animate-spin" />
                      ) : null}
                      {t("account.unblock")}
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
    </section>
  );
}
