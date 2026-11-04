"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ShareMarketOwnerSeatManager } from "./share-market/owner-workspace";
import { getClientOwnerSeatContext, type ClientShareOwnerContext } from "@/lib/client-share-market";
import { valueReadinessActor } from "@/lib/use-value-quota-readiness";
import { formatDateTime } from "@/lib/utils";

/** Mount only for the current account and selected Share. No Owner data is
 * merged into the public/buyer context or persisted in browser storage. */
export function ClientShareOwnerManagement({ shareId, actor, onClose, onChanged }: {
  shareId: string;
  actor: string;
  onClose: () => void;
  onChanged: () => Promise<void> | void;
}) {
  const { t, locale } = useLocaleText();
  const [context, setContext] = React.useState<ClientShareOwnerContext | null>(null);
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [refreshing, setRefreshing] = React.useState(false);
  const [error, setError] = React.useState(false);
  const request = React.useRef<AbortController | null>(null);
  const contextRef = React.useRef<ClientShareOwnerContext | null>(null);
  const mounted = React.useRef(false);
  const interaction = React.useRef(false);
  const interactionChanged = React.useCallback((active: boolean) => { interaction.current = active; }, []);
  const current = React.useCallback(() => mounted.current && actor === valueReadinessActor(), [actor]);
  const reload = React.useCallback(async (options?: { silent?: boolean }) => {
    if (!current()) return;
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    // Keep the open seat table in place. The first load still shows the spinner.
    const silent = options?.silent && contextRef.current != null;
    if (silent) setRefreshing(true);
    else {
      setLoading(true);
      setRefreshing(false);
      setError(false);
    }
    try {
      const next = await getClientOwnerSeatContext(shareId, controller.signal);
      if (!controller.signal.aborted && current()) {
        contextRef.current = next;
        setContext(next);
        setError(false);
      }
    } catch {
      if (!controller.signal.aborted && current()) {
        // Never leave mutation buttons or renter data from an earlier success
        // on screen after an authorization or refresh failure.
        contextRef.current = null;
        setContext(null);
        setError(true);
      }
    } finally {
      if (!controller.signal.aborted && current()) { setLoading(false); setRefreshing(false); }
      if (request.current === controller) request.current = null;
    }
  }, [shareId, current]);
  React.useEffect(() => {
    mounted.current = true;
    void reload();
    const refresh = () => {
      if (document.visibilityState === "visible" && !interaction.current && !request.current) void reload({ silent: true });
    };
    const timer = window.setInterval(refresh, 15_000);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      mounted.current = false;
      request.current?.abort();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [reload]);
  const changed = async () => {
    await reload({ silent: true });
    if (current()) await onChanged();
  };
  const listings = context?.listings ?? [];
  const selected = listings.find(listing => listing.id === selectedId)
    ?? listings.find(listing => listing.status === "active") ?? listings[0] ?? null;
  if (actor !== valueReadinessActor()) return null;
  if (context && selected) return <ShareMarketOwnerSeatManager
    listings={listings} selected={selected} refreshing={refreshing} onClose={onClose} onChanged={changed} onInteractionChange={interactionChanged}
    selector={listings.length > 1 ? <CompactSelect ariaLabel={t("clientShares.listing")} value={selected.id}
      disabled={refreshing} onChange={setSelectedId} options={listings.map(listing => ({ value: listing.id,
        label: `${t(listing.status === "active" ? "shareMarket.listings.active" : "shareMarket.closed")} · ${formatDateTime(listing.createdAt, locale)}`,
      }))} /> : undefined}
  />;
  return <Modal.Backdrop isOpen onOpenChange={open => !open && onClose()}>
    <Modal.Container placement="center">
      <Modal.Dialog className="light w-[min(760px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
        <Modal.Header><Modal.Heading>{t("clientShares.manage")}</Modal.Heading></Modal.Header>
        <Modal.Body>
          {loading ? <p role="status" className="flex items-center gap-2 text-sm"><Loader2 className="size-4 animate-spin" />{t("common.loading")}</p> : null}
          {error ? <p role="alert" className="text-sm text-danger">{t("clientShares.managementError")}</p> : null}
        </Modal.Body>
        <Modal.Footer>
          <Button variant="outline" isDisabled={loading} onPress={() => void reload()}>{t("common.reload")}</Button>
          <Button variant="ghost" onPress={onClose}>{t("common.close")}</Button>
        </Modal.Footer>
      </Modal.Dialog>
    </Modal.Container>
  </Modal.Backdrop>;
}
