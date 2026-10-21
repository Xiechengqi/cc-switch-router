"use client";

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getMarketWalletTopup,
  getMarketWalletTopupGatewayStatus,
  type MarketTopupGatewayStatus,
} from "@/lib/market-wallet-api";
import { verifiedPayableCheckoutUrl } from "@/lib/market-wallet-money";

function CheckoutRedirect() {
  const { t } = useLocaleText();
  const topupId = useSearchParams().get("topup");
  const [unavailable, setUnavailable] = React.useState(false);
  const [reason, setReason] = React.useState<MarketTopupGatewayStatus["payabilityReason"]>(undefined);

  React.useEffect(() => {
    let active = true;
    setUnavailable(false);
    setReason(undefined);
    if (!topupId) {
      setUnavailable(true);
      return;
    }
    const redirect = async () => {
      try {
        const order = await getMarketWalletTopup(topupId);
        if (!active) return;
        if (order.buyerAbandonedAt || order.status !== "pending") {
          setReason(order.buyerAbandonedAt ? "buyer_stopped" : "order_changed");
          setUnavailable(true);
          return;
        }
        const gateway = await getMarketWalletTopupGatewayStatus(topupId);
        if (!active) return;
        const serverNow = Date.parse(gateway.serverNow);
        const checkoutUrl = verifiedPayableCheckoutUrl(
          order.status,
          order.checkoutUrl,
          gateway,
          !!order.buyerAbandonedAt,
          Number.isFinite(serverNow) ? serverNow : Date.now(),
        );
        if (!checkoutUrl) {
          setReason(gateway.payabilityReason === "payable"
            ? "checkout_unavailable"
            : gateway.payabilityReason ?? "gateway_unknown");
          setUnavailable(true);
          return;
        }
        window.location.replace(checkoutUrl);
      } catch {
        if (active) {
          setReason("gateway_unknown");
          setUnavailable(true);
        }
      }
    };
    void redirect();
    return () => { active = false; };
  }, [topupId]);

  return <main className="mx-auto grid min-h-screen w-full max-w-lg content-center gap-4 px-5 py-12 text-center">
    <h1 className="text-xl font-semibold">{t("marketWallet.checkoutRedirectTitle")}</h1>
    <p role="status" className="text-sm text-muted-foreground">
      {t(unavailable ? "marketWallet.checkoutRedirectUnavailable" : "marketWallet.checkoutRedirectChecking")}
    </p>
    {unavailable && reason ? (
      <p className="text-sm text-amber-700">{t(`marketWallet.payability.${reason}` as Parameters<typeof t>[0])}</p>
    ) : null}
    <Link
      className="inline-flex min-h-11 items-center justify-center font-medium text-primary underline underline-offset-2"
      href={topupId ? `/account/wallet/?topup=${encodeURIComponent(topupId)}` : "/account/wallet/"}
    >
      {t("marketWallet.checkoutRedirectReturn")}
    </Link>
  </main>;
}

export default function WalletCheckoutPage() {
  return <React.Suspense fallback={null}><CheckoutRedirect /></React.Suspense>;
}
