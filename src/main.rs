mod api;
mod auth;
mod temp;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use api::{APP_API, Api, CLIENT_API};

#[derive(Parser)]
#[command(name = "8sleep", version, about = "Control an Eight Sleep Pod")]
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
        eprintln!("8sleep: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut api = Api::new()?;
    match cli.cmd {
        Cmd::Status => status(&mut api, cli.json),
        Cmd::On => {
            let user = target_user(&mut api, cli.side)?;
            set_state(&mut api, &user, "smart")?;
            print_side(&mut api, &user, cli.json)
        }
        Cmd::Off => {
            let user = target_user(&mut api, cli.side)?;
            set_state(&mut api, &user, "off")?;
            print_side(&mut api, &user, cli.json)
        }
        Cmd::Temp { value } => {
            let user = target_user(&mut api, cli.side)?;
            let url = format!("{APP_API}/users/{user}/temperature");
            let cur = api.get(&url)?;
            if state_type(&cur) == "off" {
                set_state(&mut api, &user, "smart")?;
            }
            // In smart mode this overrides the current phase's level
            // (overrideLevels.<phase>); `on` after `off` drops the override.
            api.put(&url, json!({ "currentLevel": value }))?;
            print_side(&mut api, &user, cli.json)
        }
        Cmd::Away { state } => {
            let user = target_user(&mut api, cli.side)?;
            // The API applies an away period immediately only when its
            // timestamp is in the past, so backdate it a day.
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
            print_side(&mut api, &user, cli.json)
        }
        Cmd::Prime => {
            let me = me(&mut api)?;
            let uid = api.user_id().to_owned();
            api.post(
                &format!("{APP_API}/devices/{}/priming/tasks", me.device_id),
                json!({ "notifications": { "users": [uid], "meta": "rePriming" } }),
            )?;
            if cli.json {
                println!("{}", json!({ "priming": true }));
            } else {
                println!("priming started");
            }
            Ok(())
        }
        Cmd::Side { side } => {
            let me = me(&mut api)?;
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
                println!("{side}");
            }
            Ok(())
        }
    }
}

struct Me {
    device_id: String,
    side: String,
}

fn me(api: &mut Api) -> Result<Me> {
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

fn device(api: &mut Api, id: &str) -> Result<Value> {
    Ok(api.get(&format!("{CLIENT_API}/devices/{id}"))?["result"].take())
}

/// The user whose temperature a command changes. Temperature is per user, so
/// `--side` picks whoever is assigned to that side of the bed.
fn target_user(api: &mut Api, side: Option<Side>) -> Result<String> {
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

fn set_state(api: &mut Api, user: &str, state: &str) -> Result<()> {
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
fn side_info(api: &mut Api, user: &str) -> Result<Value> {
    let t = api.get(&format!("{APP_API}/users/{user}/temperature"))?;
    let away = api.get(&format!("{APP_API}/users/{user}/away-mode"))?;
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
    }))
}

fn fmt_level(raw: &Value) -> String {
    match raw.as_i64() {
        Some(r) => format!(
            "{}°F ({})",
            temp::raw_to_f(r as i32),
            temp::raw_to_app(r as i32)
        ),
        None => "?".into(),
    }
}

fn side_line(label: &str, s: &Value) -> String {
    let power = if s["on"].as_bool() == Some(true) {
        "on "
    } else {
        "off"
    };
    let away = if s["away"].as_bool() == Some(true) {
        "  AWAY"
    } else {
        ""
    };
    format!(
        "{label:<6} {power}  {} → {}  [{}]{away}",
        fmt_level(&s["level"]),
        fmt_level(&s["target"]),
        s["state"].as_str().unwrap_or("?"),
    )
}

fn print_side(api: &mut Api, user: &str, as_json: bool) -> Result<()> {
    let s = side_info(api, user)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&s)?);
    } else {
        println!("{}", side_line("", &s).trim_start());
    }
    Ok(())
}

fn local_time(ts: Option<&str>, fmt: &str) -> Option<String> {
    let t: jiff::Timestamp = ts?.parse().ok()?;
    Some(
        t.to_zoned(jiff::tz::TimeZone::system())
            .strftime(fmt)
            .to_string(),
    )
}

fn status(api: &mut Api, as_json: bool) -> Result<()> {
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

    let online = if dev["online"].as_bool() == Some(true) {
        "online"
    } else {
        "OFFLINE"
    };
    let primed = local_time(dev["lastPrime"].as_str(), "%b %-d %-I:%M %p")
        .map(|t| format!(" (primed {t})"))
        .unwrap_or_default();
    println!(
        "{} · {online} · {water}{primed}",
        dev["modelString"].as_str().unwrap_or("Pod")
    );
    for s in &sides {
        let mut label = s["side"].as_str().unwrap_or("?").to_owned();
        if s["you"].as_bool() == Some(true) && label != "solo" {
            label.push('*');
        }
        if s["state"] == "unassigned" {
            println!("{label:<6} unassigned");
            continue;
        }
        println!("{}", side_line(&label, s));
    }
    if let Some(next) = sides
        .iter()
        .find(|s| s["you"].as_bool() == Some(true))
        .and_then(|s| local_time(s["next_bedtime"].as_str(), "%a %-I:%M %p"))
    {
        println!("next bedtime {next}");
    }
    Ok(())
}
