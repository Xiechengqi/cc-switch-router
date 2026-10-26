// Keep the reader ceiling independent of the ordinary descriptor version and
// market floor: adding a negotiated capability must not retire older peers.
export function auditShareContractVersions(models, protocol) {
  const errors = [];
  const versions = {};
  for (const [key, name] of [
    ["minimumVersion", "MIN_SHARE_CONTRACT_VERSION"],
    ["marketMinimumVersion", "MIN_SHARE_MARKET_CONTRACT_VERSION"],
    ["currentVersion", "SHARE_CONTRACT_VERSION"],
  ]) {
    const match = models.match(new RegExp(`^pub const ${name}:\\s*u16\\s*=\\s*(\\d+)\\s*;`, "m"));
    const value = match ? Number(match[1]) : NaN;
    if (!Number.isInteger(value) || value < 1 || value > 65535) {
      errors.push(`src/models.rs must declare ${name} as a positive u16 literal`);
    } else {
      versions[key] = value;
    }
  }
  if (errors.length) return { errors, versions };

  const { minimumVersion, marketMinimumVersion, currentVersion } = versions;
  if (minimumVersion !== 2) {
    errors.push("src/models.rs must retain the Share reader floor at v2");
  }
  if (marketMinimumVersion !== 5) {
    errors.push("src/models.rs must retain MIN_SHARE_MARKET_CONTRACT_VERSION at v5");
  }
  if (currentVersion < 8) {
    errors.push("src/models.rs Share reader must support accountReset Contract v8");
  }

  const readerRange = protocol.match(/\bShare Contract v(\d+)\.\.v(\d+)\b/);
  if (!readerRange || Number(readerRange[1]) !== minimumVersion || Number(readerRange[2]) !== currentVersion) {
    errors.push(`PROTOCOL.md must document the reader range Share Contract v${minimumVersion}..v${currentVersion}`);
  }
  if (!/\bShare Contract v7\b/.test(protocol)) {
    errors.push("PROTOCOL.md must retain ordinary Share Contract v7 compatibility");
  }
  const marketFloor = protocol.match(/`MIN_SHARE_MARKET_CONTRACT_VERSION\s*=\s*(\d+)`/);
  if (!marketFloor || Number(marketFloor[1]) !== marketMinimumVersion) {
    errors.push(`PROTOCOL.md must document MIN_SHARE_MARKET_CONTRACT_VERSION=${marketMinimumVersion}`);
  }

  return { errors, versions };
}
