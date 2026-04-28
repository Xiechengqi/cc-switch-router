# cc-switch-router Migration

This project was previously named `portr-rs`.

## Scope of the rename

- Crate/binary name: `portr-rs` -> `cc-switch-router`
- Release asset: `portr-rs-linux-amd64` -> `cc-switch-router-linux-amd64`
- Default config dir: `~/.config/portr-rs/` -> `~/.config/cc-switch-router/`
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
- env file at `~/.config/cc-switch-router/.env`
- DB path at `~/.config/cc-switch-router/cc-switch-router.db`
- host key path at `~/.config/cc-switch-router/ssh_host_ed25519_key`
- internal probe routes under `/_share-router/*`
- internal probe/error headers using `X-Share-Router-*`

Move existing deployments to the new names before upgrading to this version.

## Recommended deployment migration

1. Replace the binary with `cc-switch-router`.
2. Update systemd or process manager commands to the new binary path.
3. Move env vars from `PORTR_RS_*` to `CC_SWITCH_ROUTER_*`.
4. Move config files from `~/.config/portr-rs/` to `~/.config/cc-switch-router/`.
5. Keep the old files around until you confirm the new deployment is stable.

## Example systemd changes

Before:

```ini
EnvironmentFile=%h/.config/portr-rs/.env
ExecStart=/opt/portr-rs/portr-rs
```

After:

```ini
EnvironmentFile=%h/.config/cc-switch-router/.env
ExecStart=/opt/cc-switch-router/cc-switch-router
```

## Removal status

Legacy compatibility is removed in the current code. The entries above are
historical rename notes and migration instructions for operators upgrading from
older deployments.
