# Server Token Management V1

## Decisions
- Scope is server-only APP bearer token management for Linux and macOS daemon deployments.
- Pairing remains the default onboarding flow.
- Manual token management is CLI-only in V1. No admin HTTP API is added.
- `mycodex app devices` will gain `create --label <LABEL>` and `rotate <DEVICE_ID>`.
- Tokens remain write-only: the server stores only `token_hash`, never plaintext, and never re-shows an old token.
- `revoke` keeps its existing semantics by clearing the current token hash.
- Revoked devices cannot rotate their token; administrators must create a new device instead.
- macOS Local Host UI and mobile clients are out of scope for this V1.

## Execution Protocol
1. Only work on the first unchecked task whose dependencies are already satisfied.
2. After completing a task, run the relevant validation before checking the box.
3. After validation passes, check the task, add a short result note, and immediately continue to the next unchecked task until the checklist is complete.

## Checklist
- [x] Establish this document with locked decisions, execution protocol, checklist, and validation log.
  - Result: `docs/server-token-management-v1.md` was created and verified to contain all four required sections.
- [x] Add manual device token issuance in `apps/server/src/app_auth.rs` while continuing to persist only `token_hash`.
  - Result: `AppAuthStore::create_device` now creates an active device, returns a one-time plaintext token, and persists only the hash; `created_device_returns_token_and_persists_only_hash` passed.
- [x] Add active-device token rotation in `apps/server/src/app_auth.rs`, and reject rotation for revoked devices.
  - Result: `AppAuthStore::rotate_device_token` now replaces the active token hash, invalidates the previous token, and rejects revoked devices; `rotate_device_invalidates_previous_token` and `revoked_device_cannot_rotate_token` passed.
- [x] Extend `mycodex app devices` CLI with `create --label` and `rotate <device_id>`, and print one-time token output.
  - Result: `mycodex app devices create --label "CLI Test"` and `mycodex app devices rotate <device_id>` both printed `device_id`, `label`, `token`, and the one-time warning; `devices list` still omitted tokens.
- [x] Keep existing pairing CLI and WebSocket bearer-token auth behavior compatible.
  - Result: `approved_pairing_returns_token_once` and `authenticate_token_ignores_revoked_device` still passed, confirming pairing token issuance and bearer-token auth compatibility remained intact.
- [x] Update English and Chinese README docs with pure-server token management guidance and command examples.
  - Result: `README.md` and `README.zh-CN.md` now document pairing vs manual token management, the new `create/rotate/revoke/list` commands, and the one-time token visibility rule.
- [x] Run validation, update this document, and clear the remaining checklist items.
  - Result: CLI smoke tests for `create/rotate/list` passed, and final `cargo fmt --all`, `cargo test`, and `cargo clippy --all-targets --all-features -- -D warnings` all passed.

## Validation Log
- `cargo test created_device_returns_token_and_persists_only_hash --package mycodex`
- `cargo test rotate_device_invalidates_previous_token --package mycodex`
- `cargo test revoked_device_cannot_rotate_token --package mycodex`
- `cargo test approved_pairing_returns_token_once --package mycodex`
- `cargo test authenticate_token_ignores_revoked_device --package mycodex`
- Temporary-config CLI smoke validation:
  - `cargo run -q -p mycodex -- app --config <temp-config> devices create --label "CLI Test"`
  - `cargo run -q -p mycodex -- app --config <temp-config> devices rotate <device_id>`
  - `cargo run -q -p mycodex -- app --config <temp-config> devices list`
- Final regression validation:
  - `cargo fmt --all`
  - `cargo test`
  - `cargo clippy --all-targets --all-features -- -D warnings`
