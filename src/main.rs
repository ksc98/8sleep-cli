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
    Alarm {
        #[arg(value_parser = alarm::parse_action)]
        action: Option<alarm::Action>,
        /// Which alarm, by its number in `8sleep alarm`. Needed only when there's more than one.
        #[arg(short, long)]
        number: Option<usize>,
    },
    /// Show your side assignment, or change it (solo = whole bed).
    Side {
        #[arg(value_enum)]
        side: Option<Side>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Side {
    Left,
    Right,
    Solo,
}

impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Solo => "solo",
        }
    }
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
        Cmd::Alarm { action, number } => {
            let user = target_user(&api, cli.side)?;
            alarm_cmd(&api, &user, action, number, cli.json)
        }
        Cmd::Side { side } => {
            let me = me(&api)?;
            let side = match side {
                None => me.side,
                Some(s) => {
                    let uid = api.user_id().to_owned();
                    api.put(
                        &format!("{CLIENT_API}/users/{uid}/current-device"),
                        json!({ "id": me.device_id, "side": s.as_str() }),
                    )?;
                    s.as_str().to_owned()
                }
            };
            if cli.json {
                println!("{}", json!({ "side": side }));
            } else {
                println!("{}", ui::side_title(&side));
            }
            Ok(())
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
    let list_url = format!("{APP_API_V2}/users/{user}/alarms");
    let alarms = |v: Value| v["alarms"].as_array().cloned().unwrap_or_default();
    let list = alarms(api.get(&list_url)?);
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
    print_alarms(&alarms(api.get(&list_url)?), as_json, Some(&done))
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
        "autopilot": autopilot["isAutopilotActive"].as_bool(),
        "phases": phases(&t, &state),
        "alarm": alarms.as_ref().map_or(Value::Null, alarm::next),
    }))
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
