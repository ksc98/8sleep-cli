# 8sleep

Command-line control for an Eight Sleep Pod.

```
8sleep status                  # power, level → target, away, water, next bedtime
8sleep on | off
8sleep temp 68                 # °F (55-111)
8sleep temp -3                 # app scale (-10..+10)
8sleep away on | off
8sleep prime
8sleep side [solo|left|right]  # show or change your side assignment
```

`--side left|right|solo` targets whoever is assigned to that side. Without it,
commands act on your own account. `--json` prints JSON.

## Install

```
cargo install --path . --force
```

## Credentials

The first run reads your email and password from 1Password with
`op read op://Personal/EightSleep/{username,password}`. Point it at another
item with `EIGHTSLEEP_OP_ITEM=op://Vault/Item`, or skip 1Password by setting
`EIGHTSLEEP_EMAIL` and `EIGHTSLEEP_PASSWORD`. Accounts that sign in with
Google or Apple need a password set in the app first.

Tokens are cached in `~/.cache/8sleep/token.json` (mode 0600) and refreshed
with the refresh token, so 1Password is only asked again if a refresh fails.

## Temperature

The API stores a raw level from -100 to 100. The app shows it as -10..+10 and
as degrees through a lookup table, which this CLI uses for °F. The table comes
from [lukas-clarke/eight_sleep](https://github.com/lukas-clarke/eight_sleep),
the Home Assistant integration this CLI's API calls are based on.
