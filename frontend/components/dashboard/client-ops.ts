import { clientTunnelDisplayUrl } from "@/components/dashboard/share-dashboard-utils";
import type { DashboardClient } from "@/lib/types";

/**
 * Which Client operations the viewer may use, decided from the Router's
 * per-viewer flags. The board shows them only behind the card's "Ops" button,
 * and shows that button only when at least one is allowed:
 *
 * - owner, not rented: console, terminal, upgrade, subdomain switch
 * - tenant: console and upgrade (never the terminal on the seller's machine)
 * - owner while the Client is rented out, and everyone else: nothing
 *
 * Logs and chat are not operations; they stay public on the card.
 */
export type ClientOpsAccess = {
  console: boolean;
  terminal: boolean;
  /** May inspect and start upgrades; the upgrade item may still hide while nothing is to do. */
  upgrade: boolean;
  takeover: boolean;
};

export function clientOpsAccess(client: DashboardClient, canTakeover: boolean): ClientOpsAccess {
  const tunnel = !!clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  const rented = !!client.rental;
  // The owner of a rented-out Client hands operations to the tenant, who is
  // the only viewer with the lease's control id.
  const tenant = rented && !!client.rental?.controlId;
  if (rented && !tenant) return { console: false, terminal: false, upgrade: false, takeover: false };
  return {
    console: tunnel && client.canOpenClientConsole === true,
    terminal: tunnel && !rented && client.canOpenClientTerminal === true,
    upgrade: client.canUpgrade === true,
    // The board offers the switch from emails; the Router's owner check (the
    // console flag of an unrented Client) has the last word.
    takeover: !rented && canTakeover && client.canOpenClientConsole === true,
  };
}
