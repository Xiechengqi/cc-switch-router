/** Human-readable labels for DujiaoPay payment token ids like "tron-usdt". */

const CHAIN_NAMES: Record<string, string> = {
  tron: "TRON",
  ethereum: "Ethereum",
  bsc: "BNB Chain",
  polygon: "Polygon",
  base: "Base",
  arbitrum: "Arbitrum",
  plasma: "Plasma",
  "x-layer": "X Layer",
  solana: "Solana",
  aptos: "Aptos",
};

const SYMBOLS: Record<string, string> = {
  trx: "TRX", usdt: "USDT", usdc: "USDC", eth: "ETH", bnb: "BNB", usdt0: "USDT0",
};

export function splitPaymentTokenId(tokenId: string) {
  const index = tokenId.lastIndexOf("-");
  if (index <= 0) return { chain: tokenId, symbol: "" };
  return { chain: tokenId.slice(0, index), symbol: tokenId.slice(index + 1) };
}

/** "tron-usdt" → { symbol: "USDT", chainName: "TRON", label: "USDT · TRON" }. */
export function paymentTokenLabel(tokenId: string) {
  const { chain, symbol } = splitPaymentTokenId(tokenId);
  const chainName = CHAIN_NAMES[chain] ?? chain;
  const shownSymbol = SYMBOLS[symbol] ?? symbol.toUpperCase();
  return { symbol: shownSymbol, chainName, label: shownSymbol ? `${shownSymbol} · ${chainName}` : chainName };
}
