"use client";

import { Button, Card, Chip, Drawer } from "@heroui/react";
import { ChevronLeft, ChevronRight, ExternalLink, Maximize2 } from "lucide-react";
import * as React from "react";
import { ShareConnectDialog } from "@/components/dashboard/share-connect-dialog";
import { ShareCard } from "@/components/dashboard/share-card";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  ClientFrameDialog,
  ClientLinkedSharesPanel,
  clientOwnerEmail,
  clientPlatformLabel,
  clientTunnelDisplayUrl,
  ClientProvidersPanel,
  DrawerSection,
  drawerDialogClassName,
  EmptyBlock,
  formatAgeDaysOrHours,
  HealthDots,
  HealthTimelineStrip,
  ShareClientPanel,
  ShareEditDialog,
  ShareMarkets,
  ShareModelHealthChecks,
  ShareProvidersPanel,
  shareApiParts,
  sortClients,
} from "@/components/dashboard/data-tables";
import type { DashboardClient, DashboardMarket, ShareView } from "@/lib/types";

function sortShares(shares: ShareView[]) {
  return [...shares].sort((left, right) => {
    return (
      (Date.parse(left.createdAt) || 0) - (Date.parse(right.createdAt) || 0) ||
      (left.subdomain || left.shareName || left.shareId).localeCompare(
        right.subdomain || right.shareName || right.shareId,
        undefined,
        { sensitivity: "base" },
      )
    );
  });
}

const ShareScroller = React.memo(function ShareScroller({
  shares,
  onOpenShare,
  onEditShare,
  onConnectShare,
}: {
  shares: ShareView[];
  onOpenShare: (share: ShareView) => void;
  onEditShare: (share: ShareView) => void;
  onConnectShare: (share: ShareView) => void;
}) {
  const { t } = useLocaleText();
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  const enableWheel = shares.length >= 4;

  React.useEffect(() => {
    const element = scrollRef.current;
    if (!element || !enableWheel) return;
    const onWheel = (event: WheelEvent) => {
      if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
      event.preventDefault();
      element.scrollLeft += event.deltaY;
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [enableWheel, shares.length]);

  const scrollByCards = React.useCallback((direction: -1 | 1) => {
    scrollRef.current?.scrollBy({ left: direction * 320, behavior: "smooth" });
  }, []);

  if (!shares.length) return <EmptyBlock>{t("dashboard.noLinkedShares")}</EmptyBlock>;

  return (
    <div className="grid min-w-0 gap-2">
      {enableWheel ? (
        <div className="flex justify-end gap-1">
          <Button size="sm" variant="outline" isIconOnly className="h-7 w-7 min-w-0 rounded-md p-0" aria-label={t("dashboard.scrollSharesLeft")} onClick={() => scrollByCards(-1)}>
            <ChevronLeft className="h-3.5 w-3.5" />
          </Button>
          <Button size="sm" variant="outline" isIconOnly className="h-7 w-7 min-w-0 rounded-md p-0" aria-label={t("dashboard.scrollSharesRight")} onClick={() => scrollByCards(1)}>
            <ChevronRight className="h-3.5 w-3.5" />
          </Button>
        </div>
      ) : null}
      <div ref={scrollRef} className="flex min-w-0 snap-x gap-3 overflow-x-auto pb-2">
        {shares.map((share) => (
          <ShareCard key={share.shareId} share={share} onOpen={onOpenShare} onEdit={onEditShare} onConnect={onConnectShare} />
        ))}
      </div>
    </div>
  );
});

function ClientCard({
  client,
  shares,
  onOpenClient,
  onOpenFrame,
  onOpenShare,
  onEditShare,
  onConnectShare,
}: {
  client: DashboardClient;
  shares: ShareView[];
  onOpenClient: (client: DashboardClient) => void;
  onOpenFrame: (url: string) => void;
  onOpenShare: (share: ShareView) => void;
  onEditShare: (share: ShareView) => void;
  onConnectShare: (share: ShareView) => void;
}) {
  const { locale, t } = useLocaleText();
  const tunnelUrl = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  const owner = clientOwnerEmail(client);
  const onlineRate = client.onlineRate24h || 0;
  const onlineTitle = `${onlineRate.toFixed(1)}% online in last 24h (${client.onlineMinutes24h || 0} / 1440 min)`;

  return (
    <Card className="overflow-hidden rounded-lg border bg-white p-0 shadow-sm">
      <Card.Content className="grid gap-4 p-4">
        <div className="grid cursor-pointer gap-3 rounded-md px-1 py-0.5 transition-colors hover:bg-primary/[0.03] md:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_auto]" onClick={() => onOpenClient(client)}>
          <div className="grid min-w-0 gap-1">
            {tunnelUrl ? (
              <a href={tunnelUrl} target="_blank" rel="noopener noreferrer" data-no-row-drawer className="inline-flex min-w-0 max-w-full items-center gap-1 break-all font-mono text-xs font-semibold text-foreground underline-offset-4 hover:underline" title={tunnelUrl} onClick={(event) => event.stopPropagation()}>
                <span className="min-w-0 break-all">{tunnelUrl}</span>
                <ExternalLink className="h-3 w-3 shrink-0 text-muted-foreground" />
              </a>
            ) : (
              <strong className="break-all font-mono text-xs text-muted-foreground">{client.installation.id}</strong>
            )}
            <span className="truncate text-xs text-muted-foreground" title={owner}>{t("dashboard.owner")}: {owner}</span>
          </div>

          <div className="grid min-w-0 gap-1 text-xs text-muted-foreground sm:grid-cols-3 md:grid-cols-1 lg:grid-cols-3">
            <span className="truncate" title={client.installation.countryCode || client.installation.region || "-"}>{t("dashboard.region")}: <strong className="text-foreground">{client.installation.countryCode || client.installation.region || "-"}</strong></span>
            <span className="truncate" title={clientPlatformLabel(client)}>{t("dashboard.version")}: <strong className="text-foreground">{clientPlatformLabel(client)}</strong></span>
            <span className="truncate" title={onlineTitle}>{t("dashboard.online")}: <strong className="text-foreground">{onlineRate.toFixed(1)}% / {formatAgeDaysOrHours(client.installation.createdAt, locale)}</strong></span>
          </div>

          <div className="flex min-w-0 flex-wrap items-center justify-start gap-2 md:justify-end">
            <HealthDots entries={client.healthChecks || []} />
            <Chip size="sm" variant="tertiary">{t("dashboard.sharesCount", { count: shares.length })}</Chip>
            {tunnelUrl ? (
              <Button size="sm" variant="outline" isIconOnly className="h-7 w-7 min-w-0 rounded-md p-0" aria-label={t("dashboard.clientFrame.title")} data-no-row-drawer onClick={(event) => { event.stopPropagation(); onOpenFrame(tunnelUrl); }}>
                <Maximize2 className="h-3.5 w-3.5" />
              </Button>
            ) : null}
          </div>
        </div>

        <ShareScroller shares={shares} onOpenShare={onOpenShare} onEditShare={onEditShare} onConnectShare={onConnectShare} />
      </Card.Content>
    </Card>
  );
}

export function ClientBoard({
  clients,
  shares,
  markets,
  onChanged,
}: {
  clients: DashboardClient[];
  shares: ShareView[];
  markets: DashboardMarket[];
  onChanged?: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const [selectedClientId, setSelectedClientId] = React.useState("");
  const [selectedShareId, setSelectedShareId] = React.useState("");
  const [editingShare, setEditingShare] = React.useState<ShareView | null>(null);
  const [connectShare, setConnectShare] = React.useState<ShareView | null>(null);
  const [clientFrameUrl, setClientFrameUrl] = React.useState("");

  const sortedClients = React.useMemo(() => sortClients(clients), [clients]);
  const shareById = React.useMemo(() => new Map(shares.map((share) => [share.shareId, share])), [shares]);
  const clientById = React.useMemo(() => new Map(clients.map((client) => [client.installation.id, client])), [clients]);
  const clientByShareId = React.useMemo(() => {
    const map = new Map<string, DashboardClient>();
    clients.forEach((client) => (client.shareIds || []).forEach((shareId) => map.set(shareId, client)));
    return map;
  }, [clients]);
  const linkedShareIds = React.useMemo(() => {
    const ids = new Set<string>();
    clients.forEach((client) => (client.shareIds || []).forEach((shareId) => ids.add(shareId)));
    return ids;
  }, [clients]);

  const sharesForClient = React.useCallback(
    (client?: DashboardClient) => sortShares((client?.shareIds || []).map((id) => shareById.get(id)).filter((share): share is ShareView => !!share)),
    [shareById],
  );
  const orphanShares = React.useMemo(() => sortShares(shares.filter((share) => !linkedShareIds.has(share.shareId))), [linkedShareIds, shares]);

  const openClient = React.useCallback((client: DashboardClient) => setSelectedClientId(client.installation.id), []);
  const closeClientDrawer = React.useCallback((open: boolean) => { if (!open) setSelectedClientId(""); }, []);
  const openShare = React.useCallback((share: ShareView) => setSelectedShareId(share.shareId), []);
  const closeShareDrawer = React.useCallback((open: boolean) => { if (!open) setSelectedShareId(""); }, []);
  const openEditShare = React.useCallback((share: ShareView) => setEditingShare(share), []);
  const closeEditShare = React.useCallback(() => setEditingShare(null), []);
  const openConnectShare = React.useCallback((share: ShareView) => setConnectShare(share), []);
  const closeConnectDialog = React.useCallback((open: boolean) => { if (!open) setConnectShare(null); }, []);
  const openClientFrame = React.useCallback((url: string) => setClientFrameUrl(url), []);
  const closeClientFrame = React.useCallback((open: boolean) => { if (!open) setClientFrameUrl(""); }, []);
  const handleSaved = React.useCallback(async () => { await onChanged?.(); }, [onChanged]);

  const selectedClient = selectedClientId ? clientById.get(selectedClientId) || null : null;
  const selectedShare = selectedShareId ? shareById.get(selectedShareId) || null : null;
  const selectedClientUrl = clientTunnelDisplayUrl(selectedClient?.clientTunnel?.tunnelUrl);
  const selectedApi = shareApiParts(selectedShare ?? undefined);

  return (
    <section className="grid gap-4">
      <div className="flex items-center justify-between font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
        <div>{t("dashboard.clients")} <span className="font-semibold text-foreground">{sortedClients.length}</span></div>
        <a href="https://github.com/Xiechengqi/cc-switch/releases" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-blue-400">{t("dashboard.install")}</a>
      </div>

      <div className="grid gap-4">
        {sortedClients.length ? sortedClients.map((client) => (
          <ClientCard key={client.installation.id} client={client} shares={sharesForClient(client)} onOpenClient={openClient} onOpenFrame={openClientFrame} onOpenShare={openShare} onEditShare={openEditShare} onConnectShare={openConnectShare} />
        )) : <EmptyBlock>{t("dashboard.noClients")}</EmptyBlock>}
      </div>

      {orphanShares.length ? (
        <div className="grid gap-3">
          <div className="font-mono text-[11px] uppercase tracking-[0.14em] text-muted-foreground">
            {t("dashboard.unlinkedClients")} <span className="font-semibold text-foreground">{orphanShares.length}</span>
          </div>
          <Card className="rounded-lg border bg-white p-0 shadow-sm">
            <Card.Content className="p-4">
              <ShareScroller shares={orphanShares} onOpenShare={openShare} onEditShare={openEditShare} onConnectShare={openConnectShare} />
            </Card.Content>
          </Card>
        </div>
      ) : null}

      <Drawer isOpen={!!selectedClient} onOpenChange={closeClientDrawer}>
        <Drawer.Backdrop>
          <Drawer.Content placement="right">
            <Drawer.Dialog className={drawerDialogClassName}>
              <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Drawer.Header>
                <div>
                  <Drawer.Heading className="break-all font-mono text-base">{selectedClientUrl || "-"}</Drawer.Heading>
                  <p className="mt-1 text-sm text-muted-foreground">{clientOwnerEmail(selectedClient)}</p>
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selectedClient ? (
                  <div className="grid gap-5">
                    <HealthTimelineStrip timeline={selectedClient.healthTimeline || []} />
                    <DrawerSection label={t("dashboard.client")}>
                      <div className="grid gap-1 text-xs text-muted-foreground">
                        <span>URL: <strong className="break-all text-foreground">{selectedClientUrl || "-"}</strong></span>
                        <span>{t("dashboard.owner")}: <strong className="text-foreground">{clientOwnerEmail(selectedClient)}</strong></span>
                        <span>{t("dashboard.region")}: <strong className="text-foreground">{selectedClient.installation.countryCode || selectedClient.installation.region || "-"}</strong></span>
                        <span>{t("dashboard.version")}: <strong className="text-foreground">{clientPlatformLabel(selectedClient)}</strong></span>
                        <span>{t("dashboard.online")}: <strong className="text-foreground">{(selectedClient.onlineRate24h || 0).toFixed(1)}% / {formatAgeDaysOrHours(selectedClient.installation.createdAt, locale)}</strong></span>
                      </div>
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.linkedShares")}>
                      <ClientLinkedSharesPanel shares={sharesForClient(selectedClient)} onEdit={openEditShare} t={t} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.providers")}>
                      <ClientProvidersPanel shares={sharesForClient(selectedClient)} />
                    </DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
        </Drawer.Backdrop>
      </Drawer>

      <Drawer isOpen={!!selectedShare} onOpenChange={closeShareDrawer}>
        <Drawer.Backdrop>
          <Drawer.Content placement="right">
            <Drawer.Dialog className={drawerDialogClassName}>
              <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Drawer.Header>
                <div>
                  <Drawer.Heading className="break-all font-mono text-base">{selectedApi.apiUrl}</Drawer.Heading>
                  <p className="mt-1 break-all text-sm text-muted-foreground">{selectedShare?.ownerEmail || "-"}</p>
                  {selectedShare?.description ? <p className="mt-2 whitespace-pre-wrap break-words text-xs leading-5 text-muted-foreground">{selectedShare.description}</p> : null}
                </div>
              </Drawer.Header>
              <Drawer.Body className="overflow-y-auto">
                {selectedShare ? (
                  <div className="grid gap-5">
                    <HealthTimelineStrip timeline={selectedShare.healthTimeline} />
                    <DrawerSection label={t("dashboard.client")}>
                      <ShareClientPanel client={clientByShareId.get(selectedShare.shareId)} currentShare={selectedShare} shares={sharesForClient(clientByShareId.get(selectedShare.shareId))} onEdit={openEditShare} t={t} locale={locale} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.markets")}>
                      <ShareMarkets share={selectedShare} t={t} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.providers")}>
                      <ShareProvidersPanel share={selectedShare} />
                    </DrawerSection>
                    <DrawerSection label={t("dashboard.modelHealthChecks")}>
                      <ShareModelHealthChecks checks={selectedShare.recentModelHealthChecks || []} />
                    </DrawerSection>
                  </div>
                ) : null}
              </Drawer.Body>
            </Drawer.Dialog>
          </Drawer.Content>
        </Drawer.Backdrop>
      </Drawer>

      <ShareEditDialog share={editingShare} markets={markets} onClose={closeEditShare} onSaved={handleSaved} />
      <ShareConnectDialog share={connectShare} open={!!connectShare} onOpenChange={closeConnectDialog} />
      <ClientFrameDialog url={clientFrameUrl} open={!!clientFrameUrl} onOpenChange={closeClientFrame} t={t} />
    </section>
  );
}
