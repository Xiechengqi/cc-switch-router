"use client";

import * as React from "react";
import { Drawer } from "@heroui/react";
import { UnderlineTabs } from "@/components/common/underline-tabs";
import { C2CBuyPanel } from "@/components/dashboard/c2c-buy-panel";
import { C2COrdersPanel, type C2CView } from "@/components/dashboard/c2c-orders-panel";
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { WalletTransferPanel } from "@/components/dashboard/wallet-transfer-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";

function WalletDrawer({ open, onClose, busy, title, children }: {
  open: boolean;
  onClose: () => void;
  busy: boolean;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <Drawer.Backdrop isOpen={open} onOpenChange={(next) => { if (!next && !busy) onClose(); }}>
      <Drawer.Content placement="right">
        <Drawer.Dialog className={cn(drawerDialogClassName, "h-full !w-[min(560px,calc(100vw-0.5rem))]")}>
          {!busy ? <Drawer.CloseTrigger className="!h-11 !w-11 !bg-slate-100 !text-slate-700 hover:!bg-slate-200" /> : null}
          <Drawer.Header>
            <Drawer.Heading className="text-lg">{title}</Drawer.Heading>
          </Drawer.Header>
          <Drawer.Body className="grid content-start gap-5 overflow-y-auto pb-8">{children}</Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

export type TransferPrefill = { email: string; amount: string } | null;

export function WalletTransferDrawer({ open, onClose, prefill, walletVersion, onTransferred }: {
  open: boolean;
  onClose: () => void;
  prefill: TransferPrefill;
  walletVersion: number;
  onTransferred: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const [busy, setBusy] = React.useState(false);
  return (
    <WalletDrawer open={open} onClose={onClose} busy={busy} title={prefill ? t("walletTransfer.returnTo", { email: prefill.email }) : t("walletPage.send")}>
      {open ? (
        <WalletTransferPanel
          key={prefill ? `${prefill.email}:${prefill.amount}` : "new"}
          embedded
          inlineConfirmation
          initialRecipient={prefill?.email}
          initialAmount={prefill?.amount}
          walletVersion={walletVersion}
          onBusyChange={setBusy}
          onTransferred={onTransferred}
        />
      ) : null}
    </WalletDrawer>
  );
}

const C2C_VIEWS: C2CView[] = ["buy", "sell", "orders"];

export function WalletC2CDrawer({ open, onClose, actor, view, onViewChange, orderId, onOrderChange, onActiveCountChange }: {
  open: boolean;
  onClose: () => void;
  actor: string;
  view: C2CView;
  onViewChange: (view: C2CView) => void;
  orderId: string | null;
  onOrderChange: (id: string | null) => void;
  onActiveCountChange: (count: number) => void;
}) {
  const { t } = useLocaleText();
  const [busy, setBusy] = React.useState(false);
  return (
    <WalletDrawer open={open} onClose={onClose} busy={busy} title={t("walletPage.c2c")}>
      {open ? (
        <div className="grid min-w-0 gap-4">
          <UnderlineTabs
            value={view}
            onChange={onViewChange}
            ariaLabel={t("c2c.mode")}
            disabled={busy}
            items={C2C_VIEWS.map((item) => ({ id: item, ariaLabel: t(`c2c.tab.${item}`), label: t(`c2c.tab.${item}`) }))}
          />
          {view === "buy" ? (
            <C2CBuyPanel key={`${actor}:buy`} actor={actor} inlineConfirmation onBusyChange={setBusy} onOrder={(order) => { onViewChange("orders"); onOrderChange(order.id); }} />
          ) : null}
          <C2COrdersPanel key={actor} actor={actor} selectedId={orderId} onSelect={onOrderChange} view={view} onActiveCountChange={onActiveCountChange} />
        </div>
      ) : null}
    </WalletDrawer>
  );
}
