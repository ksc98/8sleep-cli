# 8sleep-cli

Rust CLI for the Eight Sleep Pod. Package `eightsleep`, binary `8sleep`
(Cargo rejects package names that start with a digit).

## Workflow

- After every change: `cargo test`, `cargo install --path . --force`, test the installed `~/.cargo/bin/8sleep`, then commit and push to `main` on `ksc98/8sleep-cli`.
- No CI yet. Don't add GitHub Actions or other CI until asked.

## Code map

- `src/main.rs`: clap (derive) commands (no subcommand prints help), side → user resolution, status rendering.
- `src/api.rs`: blocking HTTP client (shared across threads, HTTP/2, server error messages pulled out of the body) for `client-api.8slp.net` and `app-api.8slp.net`, retries once on 401 after a forced token refresh.
- `src/auth.rs`: password and refresh grants, 1Password lookup, atomic 0600 token cache.
- `src/alarm.rs`: alarm time and `--days` parsing (12/24 h, bare `8:30` rejected as ambiguous), summaries, next-alarm pick, vibration-test detection, server-computed fields stripped before PUT.
- `src/ui.rs`: colored rendering (owo-colors, printed via `anstream` so color drops when piped or under `NO_COLOR`), cool→warm gauge, clap help styles.
- `src/temp.rs`: raw level ↔ °F table and ↔ app scale, temperature arg parsing.

## API notes

- Reference implementation: lukas-clarke/eight_sleep `custom_components/eight_sleep/pyEight/` (`eight.py`, `user.py`, `constants.py`).
- Temperature, power and away are per user (`app-api /v1/users/{id}/...`), not per side. `--side` maps a side to the user in the device's `leftUserId`/`rightUserId`. In solo mode both are the same user.
- `8sleep schedule` (bedtime = the schedule's daily auto-on): read `schedules` from `GET app-api /v1/users/{id}/temperature/pod`, write the whole list with `PUT /v1/users/{id}/bedtime {"schedules": [...]}` (the only method it allows). New entries have no `id`. The server rejects schedules less than 16 hours apart (409 "Bedtime schedules must be at least 16 hours apart."), so in practice it's one bedtime per day. `bedtime` is an alias of `schedule`. `/v1/users/{id}/temperature/schedules` doesn't exist: `schedules` gets read as the `{specialization}` path segment (pod|pillow|all).
- Alarms: read `GET app-api /v2/users/{id}/alarms` (403 without a subscription), write the whole alarm back with `PUT /v1/users/{id}/alarms/{alarmId}` minus `alarm::SERVER_FIELDS`, create with `POST /v1/users/{id}/alarms`, delete with `DELETE /v1/users/{id}/alarms/{alarmId}`. The server's list order shifts after edits, so the CLI sorts by time and `-n` numbers come from that sort. `alarm rm` always requires `-n`.
- Scope: the user wants temperature, schedule and alarm management, kept simple. No `side` subcommand (removed at the user's request); the global `--side` flag stays. Don't grow the CLI beyond that unasked. `thermal.level` is 50 on the user's alarm; its scale is unknown, so it isn't shown as a bed level.
- The API never returns degrees for a level, only raw levels. The °F table in `temp.rs` matches a Pod 5 app's °F display (checked by the user 2026-11-19). Eight Sleep lists the Pod 6 range as 12–43 °C, close to the table's 13–44 °C. Check the table against `tempBedC` in `/v1/users/{id}/intervals` once sessions exist.
- `POST app-api /v1/users/{id}/vibration-test` (body `{}`) rings a test alarm now: it creates a one-off alarm (`pattern: TESTDRIVE`, power 100, no thermal) with a 20-minute window, which shows up in the alarms list (tag `oneOff-vibrationTest`). client-api and v2 return 404. Stop it with `PUT /v1/users/{id}/alarms/{alarmId}/dismiss` (`{"ignoreDeviceErrors": false}`), remove it with `DELETE /v1/users/{id}/alarms/{alarmId}` (verified 2026-11-19, returns the remaining alarms).
- No endpoint seen exposes power or energy use.
- The app labels the phases now / night / dawn and shows levels rounded to whole steps (`temp::raw_to_app`).
- `GET app-api /v1/users/{id}/autopilotDetails` → `isAutopilotActive`. Autopilot rewrites the phase levels overnight.
- Latency: a `PUT /temperature` takes ~0.8 s server-side even when it changes nothing (likely a synchronous round trip to the hub); GETs take ~0.1 s, plus ~0.22 s per TLS handshake. Independent reads run concurrently (`thread::scope`, `Api` is `&self`).
- Never set `content-type` on bodyless requests: the edge answers 403.
- The refresh-grant response has no `userId`. Keep the one from the password login.
- Away mode backdates the `awayPeriod` timestamp a day, as upstream does. Upstream says the change applies immediately only with a past timestamp (not tested here).
- Pod 6 returns no measured bed or room temperature on the device endpoint. Upstream reads those from `/users/{id}/trends`, which is empty until the pod has recorded a sleep session.
- Smart schedule phases: `smart.{bedTimeLevel,initialSleepLevel,finalSleepLevel}`, active one in `currentState.type` (`smart:bedtime`). `temp` in smart mode writes `overrideLevels.<phase>` for the current phase; only the `bedtime` key has been seen, `initial`/`final` are assumed to follow the state names. `off` then `on` resumes the schedule's own level and drops that override.
- Timed holds don't work on Pod 6: a `timeBased` PUT is stored but ignored while the state is `smart:*`, and a PUT of `{timeBased: {level, durationSeconds}, currentState: {type: timeBased}}` returns 500.
- Test mutations against the real bed by snapshotting `GET /users/{id}/temperature` first and restoring it after.
