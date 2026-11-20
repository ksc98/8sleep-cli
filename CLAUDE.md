# 8sleep-cli

Rust CLI for the Eight Sleep Pod. Package `eightsleep`, binary `8sleep`
(Cargo rejects package names that start with a digit).

## Workflow

- After every change: `cargo test`, `cargo install --path . --force`, test the installed `~/.cargo/bin/8sleep`, then commit and push to `main` on `ksc98/8sleep-cli`.
- No CI yet. Don't add GitHub Actions or other CI until asked.

## Code map

- `src/main.rs`: clap (derive) commands, no subcommand = `status`, side → user resolution, status rendering.
- `src/api.rs`: blocking HTTP client for `client-api.8slp.net` and `app-api.8slp.net`, retries once on 401 after a forced token refresh.
- `src/auth.rs`: password and refresh grants, 1Password lookup, atomic 0600 token cache.
- `src/ui.rs`: colored rendering (owo-colors, printed via `anstream` so color drops when piped or under `NO_COLOR`), cool→warm gauge, clap help styles.
- `src/temp.rs`: raw level ↔ °F table and ↔ app scale, temperature arg parsing.

## API notes

- Reference implementation: lukas-clarke/eight_sleep `custom_components/eight_sleep/pyEight/` (`eight.py`, `user.py`, `constants.py`).
- Temperature, power and away are per user (`app-api /v1/users/{id}/...`), not per side. `--side` maps a side to the user in the device's `leftUserId`/`rightUserId`. In solo mode both are the same user.
- Alarms: `GET app-api /v2/users/{id}/alarms` (403 without a subscription). `thermal.level` is 50 on the user's alarm; its scale is unknown, so it isn't shown as a bed level.
- The app labels the phases now / night / dawn and shows levels rounded to whole steps (`temp::raw_to_app`).
- `GET app-api /v1/users/{id}/autopilotDetails` → `isAutopilotActive`. Autopilot rewrites the phase levels overnight.
- Never set `content-type` on bodyless requests: the edge answers 403.
- The refresh-grant response has no `userId`. Keep the one from the password login.
- Away mode backdates the `awayPeriod` timestamp a day, as upstream does. Upstream says the change applies immediately only with a past timestamp (not tested here).
- Pod 6 returns no measured bed or room temperature on the device endpoint. Upstream reads those from `/users/{id}/trends`, which is empty until the pod has recorded a sleep session.
- Smart schedule phases: `smart.{bedTimeLevel,initialSleepLevel,finalSleepLevel}`, active one in `currentState.type` (`smart:bedtime`). `temp` in smart mode writes `overrideLevels.<phase>` for the current phase; only the `bedtime` key has been seen, `initial`/`final` are assumed to follow the state names. `off` then `on` resumes the schedule's own level and drops that override.
- Timed holds don't work on Pod 6: a `timeBased` PUT is stored but ignored while the state is `smart:*`, and a PUT of `{timeBased: {level, durationSeconds}, currentState: {type: timeBased}}` returns 500.
- Test mutations against the real bed by snapshotting `GET /users/{id}/temperature` first and restoring it after.
