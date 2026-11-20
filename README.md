# 8sleep

Command-line control for an Eight Sleep Pod.

```
8sleep status                  # power, target and current level, away, water, next bedtime
8sleep on | off
8sleep temp 68                 # °F (55-111)
8sleep temp -3                 # app scale (-10..+10)
8sleep away on | off
8sleep prime
8sleep alarm                   # list alarms
8sleep alarm 8:30pm            # set the alarm time (also 7am, 20:30)
8sleep alarm on | off
8sleep alarm add 7am [--days weekdays|weekends|daily|mon,wed]
8sleep alarm rm -n 2
8sleep alarm test [--for 10s]     # ring a test alarm, then stop and remove it
8sleep side [solo|left|right]  # show or change your side assignment
```

`--side left|right|solo` targets whoever is assigned to that side. Without it,
commands act on your own account. With several alarms, `-n 2` picks one by
its number in `8sleep alarm` (sorted by time of day). `--json` prints JSON. Color is off when
output is piped or `NO_COLOR` is set.

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

The API stores a raw level from -100 to 100. The CLI shows it as -10..+10
(raw ÷ 10) and as °F through a lookup table. The table comes
from [lukas-clarke/eight_sleep](https://github.com/lukas-clarke/eight_sleep),
the Home Assistant integration this CLI's API calls are based on.
