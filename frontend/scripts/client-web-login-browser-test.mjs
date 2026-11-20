import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright";

// Exercises the real Router iframe component and the built Client application
// on distinct browser origins. Only local authentication/terminal APIs are fixtures.
const frontend = fileURLToPath(new URL("../", import.meta.url));
const clientDist = process.env.CLIENT_WEB_DIST || resolve(frontend, "../../cc-switch-server/web-dist");
const temporary = await mkdtemp(resolve(tmpdir(), "client-web-login-"));
const tickets = new Map();
const tokens = new Set();
const calls = [];
let clientOrigin;
let browser;
const contentTypes = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".ttf": "font/ttf" };

const json = (response, status, value) => {
  response.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
  response.end(JSON.stringify(value));
};
const body = async (request) => {
  let text = "";
  for await (const chunk of request) text += chunk;
  return JSON.parse(text || "{}");
};
const listen = async (server) => {
  await new Promise((ready) => server.listen(0, "127.0.0.1", ready));
  return server.address().port;
};
const close = (server) => new Promise((ready) => { server.closeAllConnections(); server.close(ready); });

const client = createServer(async (request, response) => {
  try {
    const path = new URL(request.url, clientOrigin).pathname;
    const token = request.headers["x-cc-switch-client-web-session"];
    calls.push({ path, token });
    if (path === "/web-api/auth/router-exchange") {
      const input = await body(request);
      if (tickets.get(input.ticket) !== input.nonce || request.headers.origin !== clientOrigin) return json(response, 403, {});
      tickets.delete(input.ticket);
      const token = randomUUID(); tokens.add(token);
      return json(response, 200, { token, expiresAt: new Date(Date.now() + 600_000).toISOString() });
    }
    if (path === "/web-api/auth/router-logout") { tokens.delete(token); return json(response, 200, {}); }
    if (path.startsWith("/web-api/")) {
      if (!tokens.has(token)) return json(response, 401, {});
      if (path === "/web-api/context") return json(response, 200, {
        mode: "local-admin", appMode: "server", status: "authenticated", permissions: ["admin"],
        providerContract: { version: 2, minSupported: 2, maxSupported: 2 },
        runtime: { enableWebTerminal: true }, auth: { authenticated: true },
      });
      if (path === "/web-api/terminal/stream") {
        response.writeHead(200, { "content-type": "text/event-stream" });
        response.write('event: terminal\ndata: {"t":"rb"}\n\nevent: terminal\ndata: {"t":"re"}\n\n');
        const timer = setInterval(() => response.write(": keepalive\n\n"), 1_000);
        request.on("close", () => clearInterval(timer));
        return;
      }
      if (path.startsWith("/web-api/terminal/")) return json(response, 200, { ok: true });
      if (path === "/web-api/events") { response.writeHead(200, { "content-type": "text/event-stream" }); return response.end(); }
      return json(response, 200, []);
    }
    const file = resolve(clientDist, `.${path === "/" ? "/index.html" : path}`);
    if (!file.startsWith(`${resolve(clientDist)}${sep}`)) return response.writeHead(404).end();
    response.writeHead(200, { "content-type": contentTypes[extname(file)] || "application/octet-stream" });
    response.end(await readFile(file));
  } catch { response.writeHead(500).end(); }
});
const parent = createServer(async (request, response) => {
  if (request.url === "/v1/auth/client-web-tickets") {
    const input = await body(request);
    if (request.headers.authorization !== "Bearer router-long-lived-token" || input.installationId !== "client") return json(response, 403, {});
    const ticket = randomUUID(); tickets.set(ticket, input.nonce);
    return json(response, 200, { ticket, origin: clientOrigin });
  }
  if (request.url === "/app.js") {
    response.writeHead(200, { "content-type": "text/javascript" });
    return response.end(await readFile(resolve(temporary, "app.js")));
  }
  response.writeHead(200, { "content-type": "text/html" });
  response.end('<html><body><div id="root"></div><script src="/app.js"></script></body></html>');
});

try {
  clientOrigin = `http://client.router.test:${await listen(client)}`;
  const parentOrigin = `http://router.test:${await listen(parent)}`;
  const entry = `import React from 'react'; import {createRoot} from 'react-dom/client';
    import {ClientConsoleIframe} from ${JSON.stringify(resolve(frontend, "components/dashboard/client-console/client-console-iframe.tsx"))};
    localStorage.setItem('cc_switch_router_auth_v2',JSON.stringify({accessToken:'router-long-lived-token',email:'owner@example.test',expiresAt:'2100-01-01',loginEpoch:'login'}));
    createRoot(document.getElementById('root')).render(React.createElement(ClientConsoleIframe,{window:{id:'one',clientId:'client',kind:'terminal',url:${JSON.stringify(`${clientOrigin}/?view=terminal&embed=1`)},title:'Client',reloadKey:0}}));`;
  await build({ stdin: { contents: entry, resolveDir: frontend, loader: "tsx" }, outfile: resolve(temporary, "app.js"), bundle: true,
    define: { "process.env.NODE_ENV": '"production"' }, jsx: "automatic", plugins: [{ name: "fixture-contexts", setup(builder) {
      builder.onResolve({ filter: /^@\// }, (args) => {
        if (args.path === "@/components/i18n/locale-provider" || args.path.endsWith("/client-console-manager")) return { path: args.path, namespace: "fixture" };
        return builder.resolve(`./${args.path.slice(2)}`, { resolveDir: frontend, kind: args.kind });
      });
      builder.onLoad({ filter: /.*/, namespace: "fixture" }, (args) => ({ contents: args.path.includes("locale-provider")
        ? "export function useLocaleText(){return {t:(key)=>key}}"
        : "export function consoleWindowKindLabelKey(kind){return kind}" }));
    } }] });
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN || "/usr/bin/google-chrome", headless: true, args: ["--no-sandbox", "--no-proxy-server",
    "--host-resolver-rules=MAP *.router.test 127.0.0.1, MAP router.test 127.0.0.1",
    `--unsafely-treat-insecure-origin-as-secure=${parentOrigin},${clientOrigin}`] });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(parentOrigin);
  await page.waitForFunction(() => document.querySelector("iframe")?.contentWindow !== null);
  await page.waitForTimeout(1_000);
  const frame = page.frames().find((frame) => frame.url().startsWith(clientOrigin));
  assert.ok(frame);
  try { await frame.waitForSelector(".xterm", { timeout: 20_000 }); }
  catch (error) { console.error({ calls, errors, body: await frame.locator("body").innerText() }); throw error; }
  assert.ok(calls.some((call) => call.path === "/web-api/context" && tokens.has(call.token)));
  assert.ok(calls.some((call) => call.path === "/web-api/terminal/stream" && tokens.has(call.token)));
  const storage = await frame.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }));
  assert.ok(!storage.includes("router-long-lived-token"));
  for (const token of tokens) assert.ok(!storage.includes(token));
  assert.ok(!frame.url().includes("router-long-lived-token"));
  await page.evaluate(() => { localStorage.removeItem("cc_switch_router_auth_v2"); window.dispatchEvent(new Event("router-auth-changed")); });
  await page.waitForTimeout(500);
  assert.ok(calls.some((call) => call.path === "/web-api/auth/router-logout"));
  assert.deepEqual(errors, []);
  console.log("Client cross-origin owner login and terminal stream passed; logout revoked the window credential.");
} finally {
  await browser?.close();
  await Promise.all([close(parent), close(client)]);
  await rm(temporary, { recursive: true, force: true });
}
