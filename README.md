# 8sleep

Command-line control for an Eight Sleep Pod.

```
8sleep status                  # power, target and current level, schedule, alarm, recent events
8sleep status --live [10s]     # refresh in place every 5s (or the interval) until Ctrl-C
8sleep on | off
8sleep temp 68                 # °F (55-111); live while on, saved to the schedule while off
8sleep temp -3                 # app scale (-10..+10)
8sleep temp -1 --phase night   # save a phase's level in the schedule (now / night / dawn)
8sleep temp 0 --apply          # save it and run at it now, turning the bed on if needed
8sleep away on | off
8sleep prime
8sleep sleep [--since 7d]        # how you slept: stages, scores, heart rate, HRV, breathing, snoring (default: last night; --json adds every raw timeseries)
8sleep events [--since 7d]       # what changed, newest first (default: last 24h; also a date: --since 2026-11-19)
8sleep schedule                # bedtimes and the now / night / dawn levels (`bedtime` works too)
8sleep bedtime 8:30pm [--days weekdays|weekends|daily|mon,wed]
8sleep bedtime on | off
8sleep bedtime add 1am --days sat,sun
8sleep bedtime rm -n 2
8sleep alarm                   # list alarms (`alarms` works too)
8sleep alarm 8:30pm            # set the alarm time (also 7am, 20:30)
8sleep alarm on | off
8sleep alarm add 7am [--days weekdays|weekends|daily|mon,wed]
8sleep alarm rm -n 2
8sleep alarm test [--for 10s]     # ring a test alarm, then stop and remove it
```

`--side left|right|solo` targets whoever is assigned to that side. Without it,
commands act on your own account. With several alarms or bedtimes, `-n 2`
picks one by its number in the list (sorted by time of day). Eight Sleep
requires bedtimes to be at least 16 hours apart, so two daily bedtimes
can't coexist. `--json` prints JSON. Color is off when
output is piped or `NO_COLOR` is set.

## Install

```
brew install ksc98/tap/eightsleep
```

or tap first, trust the tap (Homebrew refuses short names from an untrusted
third-party tap), then install:

```
brew tap ksc98/tap
brew trust ksc98/tap
brew install eightsleep
```

or with Cargo:

```
cargo install --git https://github.com/ksc98/8sleep-cli
```

or, from a checkout, `cargo install --path .`.

## Setup

Sign in with your Eight Sleep email and password. Accounts that sign in with
Google or Apple need to set a password in the app first.

```
8sleep setup
```

prompts for both, checks them with Eight Sleep, and saves them to
`~/.config/8sleep/config.toml` (or `$XDG_CONFIG_HOME/8sleep/config.toml`), readable
only by you. `--email` skips the email prompt, and with stdin not a terminal the
password is read from its first line (`op read op://Personal/EightSleep/password |
8sleep setup --email you@example.com`).

Or write the file yourself:

```toml
email = "you@example.com"
password = "your-password"
```

and make the file readable only by you:

```
chmod 600 ~/.config/8sleep/config.toml
```

With [1Password](https://developer.1password.com/docs/cli/), name the item
instead and the CLI reads its username and password fields with `op read`:

```toml
onepassword = "op://Personal/EightSleep"
```

`EIGHTSLEEP_EMAIL` and `EIGHTSLEEP_PASSWORD` override the file.

The password is only used to sign in. The CLI then keeps a token in
`~/.cache/8sleep/token.json` (mode 0600) and refreshes it, so it signs in with the
password again only if the refresh fails.

The first run also looks up your Pod and saves its id in the same file
(`device_id = "..."`). If the Pod is ever replaced, delete that line and it's
looked up again.

## Temperature

The API stores a raw level from -100 to 100. The CLI shows it as -10..+10
(raw ÷ 10) and as °F through a lookup table. The table comes
from [lukas-clarke/eight_sleep](https://github.com/lukas-clarke/eight_sleep),
the Home Assistant integration this CLI's API calls are based on.

## License

MIT
