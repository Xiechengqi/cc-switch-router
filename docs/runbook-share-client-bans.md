# Share-scoped client ban rollout and recovery

## Contract

Router authentication failures and Share client bans are separate controls:

- Missing or invalid Router API tokens contribute only to the Router-wide authentication tracker.
- Downstream HTTP 401/403 responses never imply an authentication failure.
- A Share violation is accepted only when the trusted cc-switch-server response contains all of:

  ```text
  x-cc-switch-error-code: cc_switch_share_client_abuse
  x-cc-switch-error-scope: share
  x-cc-switch-abuse-reason: invalid_share_client_credential
  ```

  The other accepted reasons are `automated_credential_abuse` and `share_policy_abuse`.
  cc-switch-server must strip provider-supplied copies of these reserved headers.

Ten accepted violations for one `(share_id, client_ip)` within ten minutes create a one-hour
ban. A Share ban affects inference endpoints for that Share only. Router authentication bans
remain global.

## Deployment order

1. Deploy the Router fix that stops classifying arbitrary downstream 401/403 responses as
   authentication failures.
2. Deploy cc-switch-server support for the reserved typed signal. Do not emit the signal until
   its provider-response header stripping is verified.
3. Deploy Router migration 35 and the Share ban management UI. Existing active Router bans are
   in memory and disappear on restart; they are intentionally not migrated.
4. Verify logs for `typed abuse threshold reached` and confirm that the corresponding Share owner
   sees the IP only in the Share edit dialog.
5. Verify the same IP can still invoke another authorized Share.

## Verification

- Replaying ordinary provider or local-proxy 401/403 responses must not produce a ban log.
- A Share ban response is HTTP 403 with:

  ```text
  x-cc-switch-error-code: cc_switch_share_client_banned
  x-cc-switch-error-scope: share
  Retry-After: <seconds>
  ```

- A Router authentication ban instead uses `cc_switch_router_auth_client_banned` and scope
  `router`.
- `GET /v1/shares/:share_id/client-bans` is owner-only and returns `Cache-Control: no-store`.
- Unban operations are idempotent and are recorded as `share.client_ban.unban` audit events.

## Recovery and rollback

If a typed signal is found to be incorrect, stop its emission in cc-switch-server first. Existing
Share bans can be released by their owners from the Share edit dialog. Rolling Router back leaves
migration 35 and its history table in place; the older Router does not read it. Do not edit the
database manually during normal recovery.
