import { generateKeyPairSync } from "node:crypto";

// Mock API browser tests still exercise the real WebCrypto signing path.
// Replace placeholder device keys before the first application navigation.
export async function installFinancialBrowserIdentity(page) {
  const pair = generateKeyPairSync("ed25519");
  const privateKey = pair.privateKey.export({ type: "pkcs8", format: "der" }).toString("base64");
  const publicKey = pair.publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
  await page.addInitScript(({ privateKey, publicKey }) => {
    const raw = localStorage.getItem("cc_switch_router_auth_v2");
    if (raw) localStorage.setItem("cc_switch_router_auth_v2", JSON.stringify({ ...JSON.parse(raw), privateKey, publicKey }));
  }, { privateKey, publicKey });
  return pair.publicKey;
}
