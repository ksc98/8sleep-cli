//! A side's changes, from `GET app-api /v1/users/{id}/temp-events`:
//! power and dial changes, scheduled starts, schedule and level edits and
//! Autopilot's adjustments, plus the running phase's start, which no event
//! records (a bedtime moved mid-session restarts the phase in place).

use serde_json::{Value, json};

use crate::{alarm, temp, ui};

/// How far back `status` looks.
pub const STATUS_WINDOW: jiff::SignedDuration = jiff::SignedDuration::from_hours(12);
/// How many of the latest changes `status` shows.
const STATUS_SHOWN: usize = 5;

/// The event log from `from` until now.
pub fn url(user: &str, from: jiff::Timestamp) -> String {
    let iso = |t: jiff::Timestamp| t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();
    let to = jiff::Timestamp::now() + jiff::SignedDuration::from_mins(1);
    format!(
        "{}/users/{user}/temp-events?from={}&to={}",
        crate::api::APP_API,
        iso(from),
        iso(to),
    )
}

/// `--since`: a duration back from now (`12h`, `7d`, `2w`) or a local date
/// (`2026-11-19`, from its midnight).
pub fn parse_since(input: &str) -> Result<jiff::Timestamp, String> {
    if let Ok(d) = humantime::parse_duration(input) {
        let d = jiff::SignedDuration::try_from(d).map_err(|e| e.to_string())?;
        return jiff::Timestamp::now()
            .checked_sub(d)
            .map_err(|e| format!("{input}: {e}"));
    }
    let date: jiff::civil::Date = input
        .parse()
        .map_err(|_| format!("{input}: not a duration (12h, 7d) or a date (2026-11-19)"))?;
    date.to_zoned(jiff::tz::TimeZone::system())
        .map(|z| z.timestamp())
        .map_err(|e| format!("{input}: {e}"))
}

/// What `status` shows: the latest few changes, newest first, as `{time,
/// type, text}`. `side` supplies `state` and `phase_since`.
pub fn recent(log: &Value, side: &Value, from: jiff::Timestamp) -> Vec<Value> {
    let mut out = list(log, side, from, false);
    out.truncate(STATUS_SHOWN);
    out
}

/// Every event since `from`, newest first, as `{time, type, text}`. With `raw`, each also
/// carries the API's event as `raw`, and events that changed nothing shown
/// are kept with a null `text`; without it they're dropped.
pub fn list(log: &Value, side: &Value, from: jiff::Timestamp, raw: bool) -> Vec<Value> {
    let mut events: Vec<&Value> = log["events"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    events.sort_by_key(|e| e["eventTime"].as_str().and_then(parse));
    let mut out: Vec<(jiff::Timestamp, Value)> = events
        .iter()
        .filter_map(|e| {
            let at = parse(e["eventTime"].as_str()?)?;
            let text = describe(e);
            if text.is_none() && !raw {
                return None;
            }
            let mut v = json!({ "time": e["eventTime"], "type": e["actionType"], "text": text });
            if raw {
                v["raw"] = (*e).clone();
            }
            Some((at, v))
        })
        .collect();
    if let Some(phase) = phase_start(&events, side, from) {
        let at = parse(phase["time"].as_str().unwrap_or_default());
        let i = out.partition_point(|(t, _)| Some(*t) <= at);
        out.insert(i, (at.unwrap_or_default(), phase));
    }
    out.into_iter().rev().map(|(_, v)| v).collect()
}

fn parse(t: &str) -> Option<jiff::Timestamp> {
    t.parse().ok()
}

/// A level on the app's scale with one decimal when it isn't a whole step:
/// Autopilot moves in fractions (-1.4 → -1.9) that whole steps would hide.
fn app(v: &Value) -> Option<String> {
    v.as_i64().map(|l| temp::raw_to_app_precise(l as i32))
}

/// `smart:initial` -> `night`.
fn phase(state: &str) -> &str {
    ui::phase_name(state.strip_prefix("smart:").unwrap_or(state))
}

/// One event as a line of text, or `None` when it changed nothing shown.
fn describe(e: &Value) -> Option<String> {
    let kind = e["actionType"].as_str().unwrap_or("?");
    if kind == "settings-update" {
        return settings(e);
    }
    let (from, to) = (e["previousPhase"].as_str(), e["currentPhase"].as_str());
    let level = app(&e["currentLevel"]);
    let by_schedule = if kind == "smart" {
        " · by schedule"
    } else {
        ""
    };
    let text = match (from, to) {
        (Some("off"), Some(on)) if on != "off" => match level {
            Some(l) => format!("on at {l}{by_schedule}"),
            None => format!("on{by_schedule}"),
        },
        (Some(f), Some("off")) if f != "off" => format!("off{by_schedule}"),
        (Some(f), Some(t)) if f != t => format!("{} phase{by_schedule}", phase(t)),
        _ => {
            let (a, b) = (app(&e["previousLevel"])?, level?);
            if a == b {
                return None;
            }
            match kind {
                "dial-update" => format!("dial {a} → {b}"),
                "autopilot-update" => {
                    format!("Autopilot {} {a} → {b}", phase(to.unwrap_or("?")))
                }
                other => format!("{other} {a} → {b}"),
            }
        }
    };
    Some(text)
}

/// A settings edit: phase levels and bedtimes, each as `old → new`.
fn settings(e: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for (name, key) in [
        ("now", "BedTime"),
        ("night", "InitialSleep"),
        ("dawn", "FinalSleep"),
    ] {
        let (a, b) = (
            app(&e[format!("previous{key}")]),
            app(&e[format!("current{key}")]),
        );
        if let (Some(a), Some(b)) = (a, b)
            && a != b
        {
            parts.push(format!("{name} {a} → {b}"));
        }
    }
    if let (Some(before), Some(after)) = (
        e["previousSchedules"].as_array(),
        e["currentSchedules"].as_array(),
    ) {
        parts.extend(schedule_diff(before, after));
    }
    if parts.is_empty() {
        return None;
    }
    let mut text = parts.join(", ");
    if let Some(reason) = e["reason"].as_str() {
        text.push_str(&format!(" ({})", reason.replace('-', " ")));
    }
    Some(text)
}

fn schedule_diff(before: &[Value], after: &[Value]) -> Vec<String> {
    let time = |s: &Value| alarm::clock(s["time"].as_str().unwrap_or("?"));
    let days = |s: &Value| ui::days_summary(&alarm::days_map(&s["days"]));
    let mut parts = Vec::new();
    for s in after {
        let Some(old) = before.iter().find(|b| b["id"] == s["id"]) else {
            parts.push(format!("bedtime {} added", time(s)));
            continue;
        };
        if old["time"] != s["time"] {
            parts.push(format!("bedtime {} → {}", time(old), time(s)));
        }
        if old["days"] != s["days"] {
            parts.push(format!("bedtime days {} → {}", days(old), days(s)));
        }
        if old["enabled"] != s["enabled"] {
            let on = if s["enabled"] == true { "on" } else { "off" };
            parts.push(format!("bedtime {} {on}", time(s)));
        }
    }
    for s in before {
        if !after.iter().any(|a| a["id"] == s["id"]) {
            parts.push(format!("bedtime {} removed", time(s)));
        }
    }
    parts
}

/// The running phase's start (`currentState.started`), unless an event
/// that switched phases (bed on, phase change) already marks it.
fn phase_start(events: &[&Value], side: &Value, from: jiff::Timestamp) -> Option<Value> {
    let since = side["phase_since"].as_str()?;
    let at = parse(since)?;
    if at < from {
        return None;
    }
    let name = side["state"].as_str()?;
    if !name.starts_with("smart:") {
        return None;
    }
    let near = jiff::SignedDuration::from_mins(10);
    let marked = events.iter().any(|e| {
        e["previousPhase"] != e["currentPhase"]
            && e["eventTime"]
                .as_str()
                .and_then(parse)
                .is_some_and(|t| t.duration_since(at).abs() <= near)
    });
    (!marked).then(|| {
        json!({ "time": since, "type": "phase-start", "text": format!("{} phase started", phase(name)) })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(time: &str) -> Value {
        json!([{ "id": "A", "enabled": true, "time": time, "days": alarm::WEEKDAYS }])
    }

    #[test]
    fn describes_events() {
        let on = json!({"actionType": "smart", "previousPhase": "off", "currentPhase": "smart:bedtime", "currentLevel": 0});
        assert_eq!(describe(&on).unwrap(), "on at 0 · by schedule");
        let off = json!({"actionType": "dial-update", "previousPhase": "smart:bedtime", "currentPhase": "off", "previousLevel": 0});
        assert_eq!(describe(&off).unwrap(), "off");
        let dial = json!({"actionType": "dial-update", "previousPhase": "smart:bedtime", "currentPhase": "smart:bedtime", "previousLevel": 0, "currentLevel": -100});
        assert_eq!(describe(&dial).unwrap(), "dial 0 → -10");
        let auto = json!({"actionType": "autopilot-update", "previousPhase": "smart:initial", "currentPhase": "smart:initial", "previousLevel": -14, "currentLevel": -20});
        assert_eq!(describe(&auto).unwrap(), "Autopilot night -1.4 → -2");
        let moved = json!({
            "actionType": "settings-update", "previousPhase": "smart:bedtime", "currentPhase": "smart:bedtime",
            "previousBedTime": 0, "currentBedTime": 0, "previousInitialSleep": -14, "currentInitialSleep": -20,
            "previousSchedules": sched("01:40:00"), "currentSchedules": sched("02:30:00"),
            "reason": "first-night-recommendation",
        });
        assert_eq!(
            describe(&moved).unwrap(),
            "night -1.4 → -2, bedtime 1:40 AM → 2:30 AM (first night recommendation)"
        );
        let same = json!({"actionType": "settings-update", "previousSchedules": sched("01:40:00"), "currentSchedules": sched("01:40:00")});
        assert!(describe(&same).is_none());
    }

    fn at(t: &str) -> jiff::Timestamp {
        t.parse().unwrap()
    }

    #[test]
    fn adds_the_phase_start_unless_an_event_marks_it() {
        let early = at("2026-11-20T00:00:00Z");
        let side = json!({"state": "smart:bedtime", "phase_since": "2026-11-20T09:31:44Z"});
        let log = json!({"events": [
            {"eventTime": "2026-11-20T09:25:43Z", "actionType": "settings-update", "previousPhase": "smart:bedtime", "currentPhase": "smart:bedtime",
             "previousSchedules": sched("01:40:00"), "currentSchedules": sched("02:30:00")},
            {"eventTime": "2026-11-20T08:38:04Z", "actionType": "smart", "previousPhase": "off", "currentPhase": "smart:bedtime", "currentLevel": 0},
        ]});
        let texts: Vec<String> = recent(&log, &side, early)
            .iter()
            .map(|c| c["text"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            texts,
            [
                "now phase started",
                "bedtime 1:40 AM → 2:30 AM",
                "on at 0 · by schedule",
            ]
        );
        // A phase start before the window is left out.
        assert_eq!(recent(&log, &side, at("2026-11-20T09:40:00Z")).len(), 2);
        // The scheduled start four minutes before the phase start covers it.
        let side = json!({"state": "smart:bedtime", "phase_since": "2026-11-20T08:42:01Z"});
        assert_eq!(recent(&log, &side, early).len(), 2);
        // The full list keeps an event that changed nothing shown.
        let mut log = log;
        log["events"]
            .as_array_mut()
            .unwrap()
            .push(json!({"eventTime": "2026-11-20T09:00:00Z", "actionType": "settings-update"}));
        let all = list(&log, &side, early, true);
        assert_eq!(all.len(), 3);
        assert!(all[1]["text"].is_null() && all[1]["raw"].is_object());
    }
}
