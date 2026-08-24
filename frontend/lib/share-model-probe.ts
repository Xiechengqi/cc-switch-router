import type { ProviderModelProbe } from "@/lib/types";

export function shellQuote(value: string) {
  return `'${value.replace(/'/g, "'\"'\"'")}'`;
}

export function buildShareProbeCurl(
  baseUrl: string,
  probe: ProviderModelProbe,
  apiToken: string,
) {
  const url = `${baseUrl}${probe.path}`;
  const bearer = apiToken ? `Bearer ${apiToken}` : "Bearer <your-api-token>";
  const wantsSse = probe.stream || probe.responseMode !== "json";
  return [
    `curl ${wantsSse ? "-N " : ""}-sS -X ${shellQuote(probe.method)} \\`,
    `  ${shellQuote(url)} \\`,
    `  -H ${shellQuote(`Authorization: ${bearer}`)} \\`,
    ...(wantsSse ? [`  -H ${shellQuote("Accept: text/event-stream")} \\`] : []),
    `  -H ${shellQuote("Content-Type: application/json")} \\`,
    `  -d ${shellQuote(JSON.stringify(probe.body))}`,
  ].join("\n");
}
