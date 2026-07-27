# cc-switch-router Migration

This document records two separate migrations:

1. **Project rename** — `portr-rs` → `cc-switch-router` (binary, config paths, env prefix)
2. **Client replacement** — `cc-switch` desktop → `cc-switch-server` (see the last section)

## Scope of the rename

This project was previously named `portr-rs`.

- Crate/binary name: `portr-rs` -> `cc-switch-router`
- Release asset: `portr-rs-linux-amd64` -> `cc-switch-router-linux-amd64`
- Default config dir: `~/.cc-switch-router/`
- Preferred env prefix: `PORTR_RS_*` -> `CC_SWITCH_ROUTER_*`
- Preferred internal probe paths:
  - `/_portr/health` -> `/_share-router/health`
  - `/_portr/request-logs` -> `/_share-router/request-logs`
  - `/_portr/share-runtime` -> `/_share-router/share-runtime`
- Preferred internal headers:
  - `X-Portr-Probe` -> `X-Share-Router-Probe`
  - `X-Portr-Error` -> `X-Share-Router-Error`
  - `X-Portr-Error-Reason` -> `X-Share-Router-Error-Reason`

## Compatibility removed

Legacy compatibility has been removed. Deployments must use:

- `CC_SWITCH_ROUTER_*` environment variables
- env file at `~/.cc-switch-router/.env`
- DB path at `~/.cc-switch-router/cc-switch-router.db`
- host key path at `~/.cc-switch-router/ssh_host_ed25519_key`
- internal probe routes under `/_share-router/*`
- internal probe/error headers using `X-Share-Router-*`

Move existing deployments to the new names before upgrading to this version.

## Recommended deployment migration

1. Replace the binary with `cc-switch-router`.
2. Update systemd or process manager commands to the new binary path.
3. Move env vars from `PORTR_RS_*` to `CC_SWITCH_ROUTER_*`.
4. Move config files from `~/.config/portr-rs/` to `~/.cc-switch-router/`.
5. Keep the old files around until you confirm the new deployment is stable.

## Example systemd changes

Before:

```ini
EnvironmentFile=%h/.config/portr-rs/.env
ExecStart=/opt/portr-rs/portr-rs
```

After:

```ini
EnvironmentFile=%h/.cc-switch-router/.env
ExecStart=/opt/cc-switch-router/cc-switch-router
```

## Removal status

Legacy compatibility is removed in the current code. The entries above are
historical rename notes and migration instructions for operators upgrading from
older deployments.

---

# Client migration: cc-switch desktop → cc-switch-server

## What changed

The router originally served `cc-switch`, a Tauri **desktop application** that
embedded a Rust tunnel client. That client has been fully replaced by
[`cc-switch-server`](https://github.com/Xiechengqi/cc-switch-server), a headless
Rust binary with no desktop dependency.

`cc-switch-server` is now the **only supported client**. The desktop application
is no longer supported, and no compatibility path is maintained for it.

## Why

- The desktop app tied a share to a human being's workstation being powered on.
  A headless server can run unattended on any Linux host, which is what the
  Client Market supply model requires.
- Provisioning a share now means "SSH into a box and run a binary" rather than
  "install a GUI app and keep it open". This is what `install-client.sh`
  automates.
- The security model changed accordingly. The old design memo argued at length
  about embedding secrets in a redistributable desktop binary. That threat model
  no longer applies: credentials live in an operator-controlled server process,
  and the router never holds upstream provider credentials at all.

## What operators need to do

Nothing, if you are running a current deployment. The router has no
desktop-specific code paths left to disable, and no configuration keys changed as
part of this migration.

If you still have `cc-switch` desktop instances pointed at this router:

1. They will fail on protocol epoch mismatch or signature verification. There is
   no fallback.
2. Migrate each one by deploying `cc-switch-server` on a host and re-registering.
   Shares must be recreated; installation identity is not transferable between
   the two clients.

## Provisioning

`install-client.sh` in this repository downloads and initializes
`cc-switch-server` on a remote host. It is invoked automatically by the Client
Market host-provisioning flow, and can also be run manually.

## Protocol

The router ↔ `cc-switch-server` contract — registration, leases, tunnel
establishment, the control plane, and ingress identity injection — is documented
in [PROTOCOL.md](PROTOCOL.md).
