mod alarm;
mod api;
mod auth;
mod temp;
mod ui;

use anstream::{eprintln, println};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use api::{APP_API, APP_API_V2, Api, CLIENT_API};

const EXAMPLES: &str = "\
\x1b[1;92mExamples:\x1b[0m
  8sleep status           show the bed
  8sleep temp 68          set the target to 68°F (saved to the schedule while off)
  8sleep temp -3          set the target on the app's -10..+10 scale
  8sleep off
  8sleep away on
  8sleep alarm 8:30pm     move your alarm
  8sleep status --json";

#[derive(Parser)]
#[command(
    name = "8sleep",
    version,
    about = "Control an Eight Sleep Pod",
    styles = ui::HELP_STYLES,
    arg_required_else_help = true,
    after_help = EXAMPLES,
)]
struct Cli {
    /// Bed side to act on. Defaults to your own side.
    #[arg(long, global = true, value_enum)]
    side: Option<Side>,
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the bed
    Status {
        /// Keep refreshing in place until Ctrl-C, every 5s or the given interval (e.g. --live 10s).
        #[arg(long, num_args = 0..=1, default_missing_value = "5s", value_parser = humantime::parse_duration)]
        live: Option<std::time::Duration>,
    },
    /// Turn on
    ///
    /// Resumes the smart schedule.
    On,
    /// Turn off
    Off,
    /// Set the temperature
    ///
    /// °F (55-111, e.g. 68) or the app's -10..+10 scale (e.g. -3).
    ///
    /// While the bed is on, this changes the live target. While it's off, it
    /// saves the level in the schedule's `now` phase and leaves the bed off,
    /// unless --apply is given.
    Temp {
        #[arg(allow_negative_numbers = true, value_parser = temp::parse)]
        value: i32,
        /// Save the level for this phase of the schedule instead.
        #[arg(long, value_enum)]
        phase: Option<Phase>,
        /// Also turn the bed on (if it's off) and run at this level now.
        #[arg(long)]
        apply: bool,
    },
    /// Away mode on/off
    Away {
        #[arg(value_enum)]
        state: Toggle,
    },
    /// Prime the water tank
    Prime,
    /// Manage alarms
    ///
    /// List alarms, set an alarm's time (8:30pm, 7am, 20:30), or turn it on/off.
    #[command(visible_alias = "alarms", args_conflicts_with_subcommands = true)]
    Alarm {
        #[command(subcommand)]
        sub: Option<AlarmCmd>,
        /// A new time (8:30pm, 7am, 20:30), or on / off.
        #[arg(value_parser = alarm::parse_action)]
        action: Option<alarm::Action>,
        /// Which alarm, by its number in `8sleep alarm`. Needed only when there's more than one.
        #[arg(short, long)]
        number: Option<usize>,
    },
    /// Manage the bedtime schedule
    ///
    /// Show the schedule (bedtimes and the now/night/dawn levels), change a
    /// bedtime (8:30pm, 23:00) or its days, or turn it on/off.
    #[command(visible_alias = "bedtime", args_conflicts_with_subcommands = true)]
    Schedule {
        #[command(subcommand)]
        sub: Option<ScheduleCmd>,
        /// A new bedtime (8:30pm, 11pm, 23:00), or on / off.
        #[arg(value_parser = alarm::parse_action)]
        action: Option<alarm::Action>,
        /// Run on these days: weekdays, weekends, daily, or a list like mon,wed,fri.
        #[arg(long, value_parser = alarm::parse_days)]
        days: Option<Value>,
        /// Which bedtime, by its number in `8sleep schedule`. Needed only when there's more than one.
        #[arg(short, long)]
        number: Option<usize>,
    },
}

#[derive(Subcommand)]
enum ScheduleCmd {
    /// Add a bedtime (daily unless --days is given).
    Add {
        #[arg(value_parser = alarm::parse_time)]
        time: String,
        /// Run on these days: weekdays, weekends, daily, or a list like mon,wed,fri.
        #[arg(long, value_parser = alarm::parse_days)]
        days: Option<Value>,
    },
    /// Delete a bedtime by its number in `8sleep schedule`.
    Rm {
        #[arg(short, long)]
        number: usize,
    },
}

#[derive(Subcommand)]
enum AlarmCmd {
    /// Add an alarm (one-time unless --days is given).
    Add {
        #[arg(value_parser = alarm::parse_time)]
        time: String,
        /// Repeat on these days: weekdays, weekends, daily, or a list like mon,wed,fri.
        #[arg(long, value_parser = alarm::parse_days)]
        days: Option<Value>,
    },
    /// Delete an alarm by its number in `8sleep alarm`.
    Rm {
        #[arg(short, long)]
        number: usize,
    },
    /// Ring a test alarm now to feel it, then stop and remove it.
    Test {
        /// How long to let it ring (e.g. 10s, 1m).
        #[arg(long = "for", default_value = "10s", value_parser = humantime::parse_duration)]
        duration: std::time::Duration,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Phase {
    Now,
    Night,
    Dawn,
}

impl Phase {
    /// The phase's key in the API's `smart` levels.
    fn smart_key(self) -> &'static str {
        match self {
            Phase::Now => "bedTimeLevel",
            Phase::Night => "initialSleepLevel",
            Phase::Dawn => "finalSleepLevel",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Phase::Now => "now",
            Phase::Night => "night",
            Phase::Dawn => "dawn",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Side {
    Left,
    Right,
    Solo,
}

#[derive(Clone, Copy, ValueEnum)]
enum Toggle {
    On,
    Off,
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("{}", ui::error(&format!("{e:#}")));
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let api = Api::new()?;
    match cli.cmd {
        Cmd::Status { live: None } => status(&api, cli.json),
        Cmd::Status { live: Some(every) } => status_live(&api, every, cli.json),
        Cmd::On => {
            let user = target_user(&api, cli.side)?;
            print_side(&api, &user, cli.json, "turned on", || {
                set_state(&api, &user, "smart")
            })
        }
        Cmd::Off => {
            let user = target_user(&api, cli.side)?;
            print_side(&api, &user, cli.json, "turned off", || {
                set_state(&api, &user, "off")
            })
        }
        Cmd::Temp {
            value,
            phase,
            apply,
        } => {
            let user = target_user(&api, cli.side)?;
            let url = format!("{APP_API}/users/{user}/temperature");
            let cur = api.get(&url)?;
            let off = state_type(&cur) == "off";
            let level = format!(
                "{}°F ({})",
                temp::raw_to_f(value),
                temp::raw_to_app_precise(value)
            );
            // Live change: the bed is on and no phase was named.
            if !off && phase.is_none() {
                // In smart mode this overrides the running phase for tonight
                // (overrideLevels.<phase>); the schedule itself is unchanged.
                let done = format!("target set to {level}");
                return print_side(&api, &user, cli.json, &done, || {
                    api.put(&url, json!({ "currentLevel": value }))
                });
            }
            let phase = phase.unwrap_or(Phase::Now);
            let mut smart = cur["smart"].clone();
            if !smart.is_object() {
                bail!("no smart schedule levels to change");
            }
            smart[phase.smart_key()] = json!(value);
            let mut msg = format!("schedule {} set to {level}", phase.name());
            if apply {
                msg.push_str(if off {
                    ", turned on at it"
                } else {
                    ", running at it now"
                });
            } else if off {
                msg.push_str("; the bed stays off");
            }
            print_side(&api, &user, cli.json, &msg, || {
                let mut t = api.put(&url, json!({ "smart": smart }))?;
                if apply {
                    if off {
                        set_state(&api, &user, "smart")?;
                    }
                    t = api.put(&url, json!({ "currentLevel": value }))?;
                }
                Ok(t)
            })
        }
        Cmd::Away { state } => {
            let user = target_user(&api, cli.side)?;
            // Backdated a day as upstream does: it reports the away period only
            // applies immediately with a past timestamp.
            let ts = (jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24))
                .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string();
            let key = match state {
                Toggle::On => "start",
                Toggle::Off => "end",
            };
            api.put(
                &format!("{APP_API}/users/{user}/away-mode"),
                json!({ "awayPeriod": { key: ts } }),
            )?;
            let msg = match state {
                Toggle::On => "away mode on",
                Toggle::Off => "away mode off",
            };
            print_side(&api, &user, cli.json, msg, || {
                api.get(&format!("{APP_API}/users/{user}/temperature"))
            })
        }
        Cmd::Prime => {
            let device_id = device_id(&api, false)?;
            let uid = api.user_id().to_owned();
            api.post(
                &format!("{APP_API}/devices/{device_id}/priming/tasks"),
                json!({ "notifications": { "users": [uid], "meta": "rePriming" } }),
            )?;
            if cli.json {
                println!("{}", json!({ "priming": true }));
            } else {
                println!("{}", ui::ok("priming started"));
            }
            Ok(())
        }
        Cmd::Alarm {
            sub,
            action,
            number,
        } => {
            let user = target_user(&api, cli.side)?;
            match sub {
                None => alarm_cmd(&api, &user, action, number, cli.json),
                Some(AlarmCmd::Add { time, days }) => alarm_add(&api, &user, &time, days, cli.json),
                Some(AlarmCmd::Rm { number }) => alarm_rm(&api, &user, number, cli.json),
                Some(AlarmCmd::Test { duration }) => alarm_test(&api, &user, duration, cli.json),
            }
        }
        Cmd::Schedule {
            sub,
            action,
            days,
            number,
        } => {
            let user = target_user(&api, cli.side)?;
            let edit = match sub {
                Some(ScheduleCmd::Add { time, days }) => Some(ScheduleEdit::Add { time, days }),
                Some(ScheduleCmd::Rm { number }) => Some(ScheduleEdit::Rm { number }),
                None if action.is_some() || days.is_some() => Some(ScheduleEdit::Change {
                    action,
                    days,
                    number,
                }),
                None => None,
            };
            schedule_cmd(&api, &user, edit, cli.json)
        }
    }
}

fn alarm_cmd(
    api: &Api,
    user: &str,
    action: Option<alarm::Action>,
    number: Option<usize>,
    as_json: bool,
) -> Result<()> {
    let list = list_alarms(api, user)?;
    let Some(action) = action else {
        return print_alarms(&list, as_json, None);
    };
    let idx = match (number, list.len()) {
        (Some(n), len) if (1..=len).contains(&n) => n - 1,
        (Some(n), len) => bail!("there is no alarm {n} (you have {len})"),
        (None, 1) => 0,
        (None, 0) => bail!("no alarms yet; create one in the Eight Sleep app"),
        (None, len) => bail!("you have {len} alarms; pick one with --number (see `8sleep alarm`)"),
    };
    let mut body = list[idx].clone();
    let id = body["id"].as_str().context("alarm has no id")?.to_owned();
    let done = match &action {
        alarm::Action::Set(t) => {
            body["time"] = json!(t);
            format!("alarm set to {}", alarm::clock(t))
        }
        alarm::Action::Enable(on) => {
            body["enabled"] = json!(on);
            format!("alarm {}", if *on { "on" } else { "off" })
        }
    };
    if let Some(o) = body.as_object_mut() {
        for k in alarm::SERVER_FIELDS {
            o.remove(k);
        }
    }
    api.put(&format!("{APP_API}/users/{user}/alarms/{id}"), body)?;
    print_alarms(&list_alarms(api, user)?, as_json, Some(&done))
}

enum ScheduleEdit {
    Change {
        action: Option<alarm::Action>,
        days: Option<Value>,
        number: Option<usize>,
    },
    Add {
        time: String,
        days: Option<Value>,
    },
    Rm {
        number: usize,
    },
}

/// `weekDays`-style map -> the schedule API's list of day names.
fn day_names(days: &Value) -> Value {
    json!(
        alarm::WEEKDAYS
            .iter()
            .filter(|w| days[**w].as_bool() == Some(true))
            .collect::<Vec<_>>()
    )
}

/// Bedtimes live in `GET /temperature/pod` as `schedules` and are written
/// back as the whole list with `PUT /bedtime {"schedules": [...]}`; an entry
/// without an `id` is a new one. Phase levels come from `GET /temperature`.
fn schedule_cmd(api: &Api, user: &str, edit: Option<ScheduleEdit>, as_json: bool) -> Result<()> {
    // Sorted by time of day so `-n` numbers stay put.
    let read = || -> Result<Vec<Value>> {
        let v = api.get(&format!("{APP_API}/users/{user}/temperature/pod"))?;
        let mut list = v["schedules"].as_array().cloned().unwrap_or_default();
        list.sort_by(|a, b| a["time"].as_str().cmp(&b["time"].as_str()));
        Ok(list)
    };
    let mut schedules = read()?;
    let pick = |n: Option<usize>, len: usize| -> Result<usize> {
        match (n, len) {
            (Some(n), len) if (1..=len).contains(&n) => Ok(n - 1),
            (Some(n), len) => bail!("there is no bedtime {n} (you have {len})"),
            (None, 1) => Ok(0),
            (None, 0) => bail!("no bedtime yet; add one with `8sleep schedule add 11pm`"),
            (None, len) => {
                bail!("you have {len} bedtimes; pick one with --number (see `8sleep schedule`)")
            }
        }
    };
    let mut done = None;
    if let Some(edit) = edit {
        let msg = match edit {
            ScheduleEdit::Change {
                action,
                days,
                number,
            } => {
                let i = pick(number, schedules.len())?;
                let s = &mut schedules[i];
                let mut what = Vec::new();
                match &action {
                    Some(alarm::Action::Set(t)) => {
                        s["time"] = json!(t);
                        what.push(format!("set to {}", alarm::clock(t)));
                    }
                    Some(alarm::Action::Enable(on)) => {
                        s["enabled"] = json!(on);
                        what.push((if *on { "on" } else { "off" }).to_string());
                    }
                    None => {}
                }
                if let Some(d) = &days {
                    s["days"] = day_names(d);
                    what.push(format!("days: {}", ui::days_summary(d)));
                }
                format!("bedtime {}", what.join(", "))
            }
            ScheduleEdit::Add { time, days } => {
                let days = match days {
                    Some(d) => d,
                    None => alarm::parse_days("daily").map_err(anyhow::Error::msg)?,
                };
                schedules.push(json!({
                    "enabled": true,
                    "time": time,
                    "days": day_names(&days),
                    "tags": [],
                    "startSettings": {},
                }));
                format!(
                    "added a {} bedtime, {}",
                    alarm::clock(&time),
                    ui::days_summary(&days)
                )
            }
            ScheduleEdit::Rm { number } => {
                let i = pick(Some(number), schedules.len())?;
                let gone = schedules.remove(i);
                format!(
                    "deleted the {} bedtime",
                    alarm::clock(gone["time"].as_str().unwrap_or("?"))
                )
            }
        };
        api.put(
            &format!("{APP_API}/users/{user}/bedtime"),
            json!({ "schedules": schedules }),
        )?;
        done = Some(msg);
        schedules = read()?;
    }
    let shown: Vec<Value> = schedules
        .iter()
        .map(|s| {
            let mut b = bedtime_days(s);
            b["enabled"] = json!(s["enabled"].as_bool() == Some(true));
            b
        })
        .collect();
    let (t, autopilot) = std::thread::scope(|sc| {
        let ap = sc.spawn(|| api.get(&format!("{APP_API}/users/{user}/autopilotDetails")));
        let t = api.get(&format!("{APP_API}/users/{user}/temperature"));
        (
            t,
            ap.join()
                .unwrap_or_else(|_| bail!("request thread panicked")),
        )
    });
    let t = t?;
    let phases = phases(&t, state_type(&t));
    let autopilot = autopilot?["isAutopilotActive"].as_bool() == Some(true);
    if as_json {
        let out = json!({ "bedtimes": shown, "phases": phases, "autopilot": autopilot });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if let Some(d) = done {
        println!("{}\n", ui::ok(&d));
    }
    for line in ui::schedule_block(&shown, &phases, autopilot) {
        println!("{line}");
    }
    Ok(())
}

/// The user's alarms sorted by time of day, without vibration-test
/// leftovers. The server's order shifts after edits, so `-n` numbers come
/// from this sort.
fn list_alarms(api: &Api, user: &str) -> Result<Vec<Value>> {
    let v = api.get(&format!("{APP_API_V2}/users/{user}/alarms"))?;
    let mut list: Vec<Value> = v["alarms"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| !alarm::is_test(a))
        .cloned()
        .collect();
    // `HH:MM:SS` sorts correctly as text.
    list.sort_by(|a, b| a["time"].as_str().cmp(&b["time"].as_str()));
    Ok(list)
}

/// New alarms ring and warm the same way as the user's first alarm, or use
/// the app's defaults when there is none.
fn alarm_add(api: &Api, user: &str, time: &str, days: Option<Value>, as_json: bool) -> Result<()> {
    let list = list_alarms(api, user)?;
    let (vibration, thermal) = match list.first() {
        Some(a) => (a["vibration"].clone(), a["thermal"].clone()),
        None => (
            json!({ "enabled": true, "powerLevel": 50, "pattern": "RISE" }),
            json!({ "enabled": true, "level": 0 }),
        ),
    };
    let repeat = match &days {
        Some(d) => json!({ "enabled": true, "weekDays": d }),
        None => {
            json!({ "enabled": false, "weekDays": alarm::parse_days("daily").map_err(anyhow::Error::msg)? })
        }
    };
    api.post(
        &format!("{APP_API}/users/{user}/alarms"),
        json!({
            "time": time,
            "enabled": true,
            "repeat": repeat,
            "vibration": vibration,
            "thermal": thermal,
        }),
    )?;
    let done = format!("added a {} alarm", alarm::clock(time));
    print_alarms(&list_alarms(api, user)?, as_json, Some(&done))
}

fn alarm_rm(api: &Api, user: &str, number: usize, as_json: bool) -> Result<()> {
    let list = list_alarms(api, user)?;
    let a = list
        .get(number.wrapping_sub(1))
        .with_context(|| format!("there is no alarm {number} (you have {})", list.len()))?;
    let id = a["id"].as_str().context("alarm has no id")?;
    api.delete(&format!("{APP_API}/users/{user}/alarms/{id}"))?;
    let done = format!(
        "deleted the {} alarm",
        alarm::clock(a["time"].as_str().unwrap_or("?"))
    );
    print_alarms(&list_alarms(api, user)?, as_json, Some(&done))
}

/// `POST /vibration-test` creates and rings a throwaway alarm. Let it ring,
/// then dismiss and delete it (and any left over from an interrupted run).
fn alarm_test(api: &Api, user: &str, ring: std::time::Duration, as_json: bool) -> Result<()> {
    let alarms_url = format!("{APP_API}/users/{user}/alarms");
    let cleanup = || -> Result<usize> {
        let v = api.get(&format!("{APP_API_V2}/users/{user}/alarms"))?;
        let tests: Vec<String> = v["alarms"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| alarm::is_test(a))
            .filter_map(|a| a["id"].as_str().map(str::to_owned))
            .collect();
        for id in &tests {
            // Dismiss stops a ringing test; it errors when nothing rings.
            let _ = api.put(
                &format!("{alarms_url}/{id}/dismiss"),
                json!({ "ignoreDeviceErrors": false }),
            );
            api.delete(&format!("{alarms_url}/{id}"))?;
        }
        Ok(tests.len())
    };
    cleanup()?;
    api.post(&format!("{APP_API}/users/{user}/vibration-test"), json!({}))?;
    if !as_json {
        println!(
            "{}",
            ui::ok(&format!(
                "test alarm ringing for {}",
                humantime::format_duration(ring)
            ))
        );
    }
    std::thread::sleep(ring);
    let removed = cleanup()?;
    if as_json {
        println!(
            "{}",
            json!({ "rang_secs": ring.as_secs(), "removed": removed })
        );
    } else {
        println!("{}", ui::ok("stopped and removed"));
    }
    Ok(())
}

fn print_alarms(list: &[Value], as_json: bool, done: Option<&str>) -> Result<()> {
    let summaries: Vec<Value> = list.iter().map(alarm::summary).collect();
    if as_json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }
    if let Some(d) = done {
        println!("{}\n", ui::ok(d));
    }
    if summaries.is_empty() {
        println!("  no alarms");
    }
    for (i, a) in summaries.iter().enumerate() {
        println!("{}", ui::alarm_line(i + 1, a));
    }
    Ok(())
}

/// The account's device id. It only changes if the pod is replaced, so it's
/// cached in `~/.cache/8sleep/device.json` (keyed by user) and `users/me` is
/// read only when the cache is missing or `refresh` is set.
fn device_id(api: &Api, refresh: bool) -> Result<String> {
    let path = auth::cache_path("device.json")?;
    if !refresh
        && let Ok(raw) = std::fs::read(&path)
        && let Ok(c) = serde_json::from_slice::<Value>(&raw)
        && c["user_id"] == api.user_id()
        && let Some(id) = c["device_id"].as_str()
    {
        return Ok(id.to_owned());
    }
    let v = api.get(&format!("{CLIENT_API}/users/me"))?;
    let id = v["user"]["currentDevice"]["id"]
        .as_str()
        .context("account has no current device")?
        .to_owned();
    let cache = json!({ "user_id": api.user_id(), "device_id": id });
    auth::write_private(&path, &serde_json::to_vec(&cache)?)?;
    Ok(id)
}

/// The account's device. If the read fails, the cached id may be stale (a
/// replaced pod), so the id is looked up again and the read retried once
/// when it changed.
fn device(api: &Api) -> Result<Value> {
    let id = device_id(api, false)?;
    let read = |id: &str| -> Result<Value> {
        Ok(api.get(&format!("{CLIENT_API}/devices/{id}"))?["result"].take())
    };
    match read(&id) {
        Ok(dev) => Ok(dev),
        Err(e) => match device_id(api, true) {
            Ok(fresh) if fresh != id => read(&fresh),
            _ => Err(e),
        },
    }
}

/// Which side `user` is on: `solo` when the same user has both sides.
fn side_of(dev: &Value, user: &str) -> &'static str {
    let (l, r) = (dev["leftUserId"].as_str(), dev["rightUserId"].as_str());
    match (l == Some(user), r == Some(user)) {
        (true, true) => "solo",
        (true, false) => "left",
        (false, true) => "right",
        _ => "unassigned",
    }
}

/// The user whose temperature a command changes. Temperature is per user, so
/// `--side` picks whoever is assigned to that side of the bed.
fn target_user(api: &Api, side: Option<Side>) -> Result<String> {
    let Some(side) = side else {
        return Ok(api.user_id().to_owned());
    };
    let dev = device(api)?;
    let left = dev["leftUserId"].as_str();
    let right = dev["rightUserId"].as_str();
    let user = match side {
        Side::Left => left.context("no one is assigned to the left side")?,
        Side::Right => right.context("no one is assigned to the right side")?,
        Side::Solo => match (left, right) {
            (Some(l), Some(r)) if l == r => l,
            _ => bail!("the bed is split; use --side left or --side right"),
        },
    };
    Ok(user.to_owned())
}

/// Switch the bed on (`smart`) or off. Returns the temperature object, which
/// the PUT answers with.
fn set_state(api: &Api, user: &str, state: &str) -> Result<Value> {
    api.put(
        &format!("{APP_API}/users/{user}/temperature"),
        json!({ "currentState": { "type": state } }),
    )
}

fn state_type(temp: &Value) -> &str {
    temp["currentState"]["type"].as_str().unwrap_or("unknown")
}

/// One user's side as `status` shows it.
fn side_info(api: &Api, user: &str) -> Result<Value> {
    side_with(api, user, || {
        api.get(&format!("{APP_API}/users/{user}/temperature"))
    })
}

/// One user's side, with `temperature` supplying the temperature object:
/// a GET, or the response of a command's own write (a PUT to
/// `/temperature` answers with the full object, so nothing is re-read). It
/// runs concurrently with the other reads, each a full API round trip.
fn side_with(api: &Api, user: &str, temperature: impl FnOnce() -> Result<Value>) -> Result<Value> {
    let (t, away, autopilot, alarms) = std::thread::scope(|sc| {
        let get = |url: String| sc.spawn(move || api.get(&url));
        let away = get(format!("{APP_API}/users/{user}/away-mode"));
        let autopilot = get(format!("{APP_API}/users/{user}/autopilotDetails"));
        let alarms = get(format!("{APP_API_V2}/users/{user}/alarms"));
        let t = temperature();
        (t, join(away), join(autopilot), join(alarms))
    });
    let (t, away, autopilot) = (t?, away?, autopilot?);
    // Accounts without a subscription get 403 from the alarms API.
    let alarms = alarms.ok();
    let state = state_type(&t).to_owned();
    let level = t["currentDeviceLevel"].as_i64().map(|v| v as i32);
    let target = t["currentLevel"].as_i64().map(|v| v as i32);
    Ok(json!({
        "on": state != "off",
        "state": state,
        "level": level,
        "level_f": level.map(temp::raw_to_f),
        "target": target,
        "target_f": target.map(temp::raw_to_f),
        "away": away["isAway"].as_bool(),
        "next_bedtime": t["nextSchedule"]["nextTimestamp"].as_str(),
        "bedtime": bedtime(&t["nextSchedule"]),
        "autopilot": autopilot["isAutopilotActive"].as_bool(),
        "phases": phases(&t, &state),
        "alarm": alarms.as_ref().map_or(Value::Null, alarm::next),
    }))
}

/// The schedule's daily auto-start: `{time, days}` with days as the
/// `weekDays` map alarms use, or null when the schedule is off.
fn bedtime(sched: &Value) -> Value {
    if sched["enabled"].as_bool() != Some(true) {
        return Value::Null;
    }
    bedtime_days(sched)
}

fn bedtime_days(sched: &Value) -> Value {
    let days: serde_json::Map<String, Value> = alarm::WEEKDAYS
        .iter()
        .map(|d| {
            let on = sched["days"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == d));
            (d.to_string(), Value::Bool(on))
        })
        .collect();
    json!({ "time": sched["time"], "days": days })
}

/// The smart schedule's three phases, in order: (state suffix and
/// `overrideLevels` key, `smart` key).
const PHASES: [(&str, &str); 3] = [
    ("bedtime", "bedTimeLevel"),
    ("initial", "initialSleepLevel"),
    ("final", "finalSleepLevel"),
];

/// Each phase's effective level. A `temp` change during a phase lands in
/// `overrideLevels.<phase>` and wins over the schedule's level.
fn phases(t: &Value, state: &str) -> Value {
    let active = state.strip_prefix("smart:");
    PHASES
        .iter()
        .map(|&(name, key)| {
            let scheduled = t["smart"][key].as_i64();
            let overridden = t["overrideLevels"][name].as_i64();
            let level = overridden.or(scheduled);
            json!({
                "phase": name,
                "level": level,
                "level_f": level.map(|l| temp::raw_to_f(l as i32)),
                "scheduled": scheduled,
                "overridden": overridden.is_some() && overridden != scheduled,
                "active": active == Some(name),
            })
        })
        .collect()
}

/// Make a change and show the side as it is after it. `change` returns the
/// user's temperature object (a write's response, or a GET after a write
/// elsewhere); the side's other reads and the device read run meanwhile.
fn print_side(
    api: &Api,
    user: &str,
    as_json: bool,
    done: &str,
    change: impl FnOnce() -> Result<Value>,
) -> Result<()> {
    let (s, dev) = std::thread::scope(|sc| {
        let dev = sc.spawn(|| device(api));
        let s = side_with(api, user, change);
        (s, join(dev))
    });
    let s = s?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    let side = side_of(&dev?, user);
    println!("{}\n", ui::ok(done));
    let title = (side != "solo").then(|| ui::side_title(side));
    if title.is_none() {
        println!("{}", ui::power_row(&s));
    }
    for line in ui::side_block(title, false, &s) {
        println!("{line}");
    }
    Ok(())
}

fn join<T>(h: std::thread::ScopedJoinHandle<'_, Result<T>>) -> Result<T> {
    h.join()
        .unwrap_or_else(|_| bail!("request thread panicked"))
}

/// Everything `status` shows, as JSON and as rendered lines.
///
/// One round of concurrent reads: the device (through the cached id) and
/// your own side. Only a bed shared with someone else needs a second round
/// for their side.
fn status_snapshot(api: &Api) -> Result<(Value, Vec<String>)> {
    let me = api.user_id();
    let (dev, mine) = std::thread::scope(|sc| {
        let dev = sc.spawn(|| device(api));
        let mine = side_info(api, me);
        (join(dev), mine)
    });
    let dev = dev?;
    let left = dev["leftUserId"].as_str().map(str::to_owned);
    let right = dev["rightUserId"].as_str().map(str::to_owned);

    let assignments: Vec<(&str, Option<String>)> = match (&left, &right) {
        (Some(l), Some(r)) if l == r => vec![("solo", Some(l.clone()))],
        _ => vec![("left", left.clone()), ("right", right.clone())],
    };
    // Your side is already fetched; anyone else's is read now, concurrently.
    let mut mine = Some(mine);
    let infos: Vec<Result<Value>> = std::thread::scope(|sc| {
        let others: Vec<_> = assignments
            .iter()
            .map(|(_, user)| match user.as_deref() {
                Some(u) if u != me => Some(sc.spawn(move || side_info(api, u))),
                _ => None,
            })
            .collect();
        assignments
            .iter()
            .zip(others)
            .map(|((_, user), other)| match (user, other) {
                (_, Some(h)) => join(h),
                (Some(_), None) => mine.take().context("your side is listed twice")?,
                (None, None) => Ok(json!({ "on": false, "state": "unassigned" })),
            })
            .collect()
    });
    let mut sides = Vec::new();
    for ((label, user), info) in assignments.iter().zip(infos) {
        let mut info = info?;
        info["side"] = json!(label);
        info["you"] = json!(user.as_deref() == Some(me));
        sides.push(info);
    }

    let water = if dev["priming"].as_bool() == Some(true) {
        "priming"
    } else if dev["needsPriming"].as_bool() == Some(true) {
        "needs priming"
    } else if dev["hasWater"].as_bool() == Some(false) {
        "no water"
    } else {
        "water ok"
    };

    // A solo bed has one block, so its power gets a row of its own right
    // under the header instead of a titled block.
    let solo = sides.len() == 1;
    let mut lines = vec![ui::device_line(&dev, water)];
    for s in &sides {
        let side = s["side"].as_str().unwrap_or("?");
        let you = !solo && s["you"].as_bool() == Some(true);
        lines.push(if solo {
            ui::power_row(s)
        } else {
            String::new()
        });
        lines.extend(ui::side_block(
            (!solo).then(|| ui::side_title(side)),
            you,
            s,
        ));
    }
    let out = json!({
        "model": dev["modelString"],
        "online": dev["online"],
        "water": water,
        "last_prime": dev["lastPrime"],
        "sides": sides,
    });
    Ok((out, lines))
}

fn status(api: &Api, as_json: bool) -> Result<()> {
    let (out, lines) = status_snapshot(api)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
}

/// Redraw `status` in place every `every` until Ctrl-C, like `watch`. With
/// `--json`, print one compact JSON object per refresh instead. A failed
/// refresh keeps the last frame and shows the error in the footer.
fn status_live(api: &Api, every: std::time::Duration, as_json: bool) -> Result<()> {
    use std::io::{IsTerminal, Write};
    if every < std::time::Duration::from_secs(2) {
        bail!("--live interval must be at least 2s");
    }
    if !as_json && !std::io::stdout().is_terminal() {
        bail!("--live needs a terminal; use --json --live to stream JSON");
    }
    let mut last: Vec<String> = Vec::new();
    loop {
        let started = std::time::Instant::now();
        let snap = status_snapshot(api);
        if as_json {
            match snap {
                Ok((v, _)) => println!("{v}"),
                Err(e) => eprintln!("{}", ui::error(&format!("{e:#}"))),
            }
        } else {
            let footer = match snap {
                Ok((_, lines)) => {
                    last = lines;
                    ui::live_footer(every, None)
                }
                Err(e) => ui::live_footer(every, Some(&format!("{e:#}"))),
            };
            // Home the cursor and clear the screen, then draw the frame.
            let mut frame = String::from("\x1b[H\x1b[2J");
            for line in last.iter().chain([&String::new(), &footer]) {
                frame.push_str(line);
                frame.push('\n');
            }
            let mut out = anstream::stdout();
            out.write_all(frame.as_bytes())?;
            out.flush()?;
        }
        std::thread::sleep(every.saturating_sub(started.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_apply_overrides_and_mark_active() {
        let t = json!({
            "smart": { "bedTimeLevel": 0, "initialSleepLevel": -14, "finalSleepLevel": 3 },
            "overrideLevels": { "bedtime": -60 },
        });
        let p = phases(&t, "smart:bedtime");
        assert_eq!(p[0]["level"], -60);
        assert_eq!(p[0]["overridden"], true);
        assert_eq!(p[0]["active"], true);
        assert_eq!(p[1]["level"], -14);
        assert_eq!(p[1]["overridden"], false);
        assert_eq!(p[1]["active"], false);
        assert!(
            phases(&t, "off")
                .as_array()
                .unwrap()
                .iter()
                .all(|x| x["active"] == false)
        );
    }
}
