# 8sleep-cli

Rust CLI for the Eight Sleep Pod. Package `eightsleep`, binary `8sleep`
(Cargo rejects package names that start with a digit).

## Workflow

- After every change: `cargo test`, `cargo install --path . --force`, test the installed `~/.cargo/bin/8sleep`, then commit and push to `main` on `ksc98/8sleep-cli`.
- No CI yet. Don't add GitHub Actions or other CI until asked.

## Code map

- `src/main.rs`: clap commands, side → user resolution, status rendering.
- `src/api.rs`: blocking HTTP client for `client-api.8slp.net` and `app-api.8slp.net`, retries once on 401 after a forced token refresh.
- `src/auth.rs`: password and refresh grants, 1Password lookup, atomic 0600 token cache.
- `src/temp.rs`: raw level ↔ °F table and ↔ app scale, temperature arg parsing.

## API notes

- Reference implementation: lukas-clarke/eight_sleep `custom_components/eight_sleep/pyEight/` (`eight.py`, `user.py`, `constants.py`).
- Temperature, power and away are per user (`app-api /v1/users/{id}/...`), not per side. `--side` maps a side to the user in the device's `leftUserId`/`rightUserId`. In solo mode both are the same user.
- Never set `content-type` on bodyless requests: the edge answers 403.
- The refresh-grant response has no `userId`. Keep the one from the password login.
- Away mode applies immediately only when the `awayPeriod` timestamp is in the past, so it is backdated a day.
- Pod 6 returns no measured bed or room temperature on the device endpoint. Upstream reads those from `/users/{id}/trends`, which is empty until the pod has recorded a sleep session.
- Test mutations against the real bed by snapshotting `GET /users/{id}/temperature` first and restoring it after.
