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
  8sleep temp 68          set the target to 68°F
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
    /// Show power, temperature, away mode and water for the bed.
    Status,
    /// Turn on (resumes the smart schedule).
    On,
    /// Turn off.
    Off,
    /// Set temperature: °F (55-111, e.g. 68) or the app's -10..+10 scale (e.g. -3).
    Temp {
        #[arg(allow_negative_numbers = true, value_parser = temp::parse)]
        value: i32,
    },
    /// Turn away mode on or off.
    Away {
        #[arg(value_enum)]
        state: Toggle,
    },
    /// Start priming the water tank.
    Prime,
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
        Cmd::Status => status(&api, cli.json),
        Cmd::On => {
            let user = target_user(&api, cli.side)?;
            set_state(&api, &user, "smart")?;
            print_side(&api, &user, cli.json, "turned on")
        }
        Cmd::Off => {
            let user = target_user(&api, cli.side)?;
            set_state(&api, &user, "off")?;
            print_side(&api, &user, cli.json, "turned off")
        }
        Cmd::Temp { value } => {
            let user = target_user(&api, cli.side)?;
            let url = format!("{APP_API}/users/{user}/temperature");
            let cur = api.get(&url)?;
            if state_type(&cur) == "off" {
                set_state(&api, &user, "smart")?;
            }
            // In smart mode this overrides the current phase's level
            // (overrideLevels.<phase>); `on` after `off` drops the override.
            api.put(&url, json!({ "currentLevel": value }))?;
            let msg = format!(
                "target set to {}°F ({})",
                temp::raw_to_f(value),
                temp::raw_to_app_precise(value)
            );
            print_side(&api, &user, cli.json, &msg)
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
            print_side(&api, &user, cli.json, msg)
        }
        Cmd::Prime => {
            let me = me(&api)?;
            let uid = api.user_id().to_owned();
            api.post(
                &format!("{APP_API}/devices/{}/priming/tasks", me.device_id),
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

struct Me {
    device_id: String,
    side: String,
}

fn me(api: &Api) -> Result<Me> {
    let v = api.get(&format!("{CLIENT_API}/users/me"))?;
    let dev = &v["user"]["currentDevice"];
    Ok(Me {
        device_id: dev["id"]
            .as_str()
            .context("account has no current device")?
            .to_owned(),
        side: dev["side"].as_str().unwrap_or("unknown").to_owned(),
    })
}

fn device(api: &Api, id: &str) -> Result<Value> {
    Ok(api.get(&format!("{CLIENT_API}/devices/{id}"))?["result"].take())
}

/// The user whose temperature a command changes. Temperature is per user, so
/// `--side` picks whoever is assigned to that side of the bed.
fn target_user(api: &Api, side: Option<Side>) -> Result<String> {
    let Some(side) = side else {
        return Ok(api.user_id().to_owned());
    };
    let me = me(api)?;
    let dev = device(api, &me.device_id)?;
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

fn set_state(api: &Api, user: &str, state: &str) -> Result<()> {
    api.put(
        &format!("{APP_API}/users/{user}/temperature"),
        json!({ "currentState": { "type": state } }),
    )?;
    Ok(())
}

fn state_type(temp: &Value) -> &str {
    temp["currentState"]["type"].as_str().unwrap_or("unknown")
}

/// One user's temperature and away state as a JSON object.
fn side_info(api: &Api, user: &str) -> Result<Value> {
    // Independent reads, fetched concurrently: each is a full API round trip.
    let (t, away, autopilot, alarms) = std::thread::scope(|sc| {
        let get = |url: String| sc.spawn(move || api.get(&url));
        let t = get(format!("{APP_API}/users/{user}/temperature"));
        let away = get(format!("{APP_API}/users/{user}/away-mode"));
        let autopilot = get(format!("{APP_API}/users/{user}/autopilotDetails"));
        let alarms = get(format!("{APP_API_V2}/users/{user}/alarms"));
        let join = |h: std::thread::ScopedJoinHandle<'_, Result<Value>>| {
            h.join()
                .unwrap_or_else(|_| bail!("request thread panicked"))
        };
        (join(t), join(away), join(autopilot), join(alarms))
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

/// Confirm a change, then show the side as it is now.
fn print_side(api: &Api, user: &str, as_json: bool, done: &str) -> Result<()> {
    let (s, me) = std::thread::scope(|sc| {
        let me = sc.spawn(|| me(api));
        let s = side_info(api, user);
        (
            s,
            me.join()
                .unwrap_or_else(|_| bail!("request thread panicked")),
        )
    });
    let s = s?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    let me = me?;
    println!("{}\n", ui::ok(done));
    for line in ui::side_block(ui::side_title(&me.side), false, &s) {
        println!("{line}");
    }
    Ok(())
}

fn status(api: &Api, as_json: bool) -> Result<()> {
    let me = me(api)?;
    let dev = device(api, &me.device_id)?;
    let left = dev["leftUserId"].as_str().map(str::to_owned);
    let right = dev["rightUserId"].as_str().map(str::to_owned);

    let mut sides = Vec::new();
    let assignments: Vec<(&str, Option<String>)> = match (&left, &right) {
        (Some(l), Some(r)) if l == r => vec![("solo", Some(l.clone()))],
        _ => vec![("left", left.clone()), ("right", right.clone())],
    };
    for (label, user) in assignments {
        let mut info = match &user {
            Some(u) => side_info(api, u)?,
            None => json!({ "on": false, "state": "unassigned" }),
        };
        info["side"] = json!(label);
        info["you"] = json!(user.as_deref() == Some(api.user_id()));
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

    if as_json {
        let out = json!({
            "model": dev["modelString"],
            "online": dev["online"],
            "water": water,
            "last_prime": dev["lastPrime"],
            "sides": sides,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("{}", ui::device_line(&dev, water));
    for s in &sides {
        let side = s["side"].as_str().unwrap_or("?");
        let you = side != "solo" && s["you"].as_bool() == Some(true);
        println!();
        for line in ui::side_block(ui::side_title(side), you, s) {
            println!("{line}");
        }
    }
    Ok(())
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
