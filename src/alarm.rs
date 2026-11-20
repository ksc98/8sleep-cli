//! Alarms: read from `GET /v2/users/{id}/alarms`, written back whole with
//! `PUT /v1/users/{id}/alarms/{alarmId}`.

use serde_json::{Value, json};

/// Fields the server computes. They are dropped before a PUT.
pub const SERVER_FIELDS: [&str; 5] = [
    "nextTimestamp",
    "startTimestamp",
    "endTimestamp",
    "dismissedUntil",
    "snoozedUntil",
];

/// Tag on the throwaway alarm `POST /vibration-test` creates.
pub const TEST_TAG: &str = "oneOff-vibrationTest";

pub fn is_test(a: &Value) -> bool {
    a["tags"]
        .as_array()
        .is_some_and(|t| t.iter().any(|t| t == TEST_TAG))
}

pub const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// The schedule API's list of day names as a `weekDays`-style map.
pub fn days_map(list: &Value) -> Value {
    WEEKDAYS
        .iter()
        .map(|d| {
            let on = list.as_array().is_some_and(|a| a.iter().any(|x| x == d));
            (d.to_string(), Value::Bool(on))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// `weekdays`, `weekends`, `daily`, or a comma list like `mon,wed,fri`, as
/// the API's `weekDays` map.
pub fn parse_days(input: &str) -> Result<Value, String> {
    let on: Vec<bool> = match input.to_ascii_lowercase().as_str() {
        "daily" | "everyday" | "every-day" => vec![true; 7],
        "weekdays" => (0..7).map(|i| i < 5).collect(),
        "weekends" => (0..7).map(|i| i >= 5).collect(),
        list => {
            let mut on = vec![false; 7];
            for d in list.split(',').map(str::trim).filter(|d| !d.is_empty()) {
                let i = WEEKDAYS
                    .iter()
                    .position(|w| d.len() >= 2 && w.starts_with(d))
                    .ok_or_else(|| {
                        format!("{d}: not a day (use mon,tue,... or weekdays/weekends/daily)")
                    })?;
                on[i] = true;
            }
            if !on.contains(&true) {
                return Err(format!("{input}: no days given"));
            }
            on
        }
    };
    Ok(WEEKDAYS
        .iter()
        .zip(on)
        .map(|(d, on)| (d.to_string(), Value::Bool(on)))
        .collect::<serde_json::Map<_, _>>()
        .into())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Set the time, as `HH:MM:00`.
    Set(String),
    Enable(bool),
}

/// `on`, `off`, or a time (see [`parse_time`]).
pub fn parse_action(s: &str) -> Result<Action, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" => Ok(Action::Enable(true)),
        "off" => Ok(Action::Enable(false)),
        _ => parse_time(s).map(Action::Set),
    }
}

/// Parse a clock time into the API's `HH:MM:00`.
///
/// Accepts `8:30pm`, `8:30 PM`, `8pm`, `830pm` and 24-hour `20:30` / `07:00`.
/// A bare `8:30` could be morning or evening, so it's rejected unless written
/// with a leading zero (`08:30`) or as an hour past 12.
pub fn parse_time(input: &str) -> Result<String, String> {
    let s: String = input.to_ascii_lowercase().split_whitespace().collect();
    let err = || format!("{input}: use a time like 8:30pm, 7am or 20:30");
    let (body, meridiem) = if let Some(b) = s.strip_suffix("am").or(s.strip_suffix('a')) {
        (b, Some(false))
    } else if let Some(b) = s.strip_suffix("pm").or(s.strip_suffix('p')) {
        (b, Some(true))
    } else {
        (s.as_str(), None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h, m),
        Some(_) => return Err(err()),
        // `830pm` / `8pm`
        None if meridiem.is_some() && body.len() > 2 => body.split_at(body.len() - 2),
        None if meridiem.is_some() => (body, "00"),
        None => return Err(err()),
    };
    let hour: u32 = h.parse().map_err(|_| err())?;
    let min: u32 = m.parse().map_err(|_| err())?;
    if min > 59 {
        return Err(err());
    }
    let hour = match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&hour) {
                return Err(err());
            }
            (hour % 12) + if pm { 12 } else { 0 }
        }
        None => {
            if hour > 23 {
                return Err(err());
            }
            if (1..=12).contains(&hour) && !h.starts_with('0') {
                return Err(format!(
                    "{input} could be morning or evening: say {hour}:{min:02}am or {hour}:{min:02}pm"
                ));
            }
            hour
        }
    };
    Ok(format!("{hour:02}:{min:02}:00"))
}

/// `09:15:00` -> `9:15 AM`.
pub fn clock(time: &str) -> String {
    let mut parts = time.split(':').map(|p| p.parse::<u32>().ok());
    let (Some(Some(h)), Some(Some(m))) = (parts.next(), parts.next()) else {
        return time.to_string();
    };
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{m:02} {}", if h < 12 { "AM" } else { "PM" })
}

/// One alarm in the shape `status`, `alarm` and `--json` share.
pub fn summary(a: &Value) -> Value {
    let repeat = a["repeat"]["enabled"].as_bool() == Some(true);
    json!({
        "id": a["id"],
        "enabled": a["enabled"].as_bool() == Some(true),
        "time": a["time"],
        "next": a["nextTimestamp"],
        "repeat": repeat,
        "days": if repeat { a["repeat"]["weekDays"].clone() } else { Value::Null },
        "vibration": a["vibration"]["enabled"].as_bool() == Some(true),
        "vibration_power": a["vibration"]["powerLevel"],
        "vibration_pattern": a["vibration"]["pattern"],
        "thermal": a["thermal"]["enabled"].as_bool() == Some(true),
        "thermal_level": a["thermal"]["level"],
        "snoozing": a["snoozing"].as_bool() == Some(true),
    })
}

/// The soonest enabled alarm that hasn't finished ringing.
pub fn next(resp: &Value) -> Value {
    let now = jiff::Timestamp::now();
    let ts = |v: &Value| v.as_str().and_then(|s| s.parse::<jiff::Timestamp>().ok());
    let candidates = resp["alarms"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(std::iter::once(&resp["recommendedAlarm"]));
    candidates
        .filter(|a| a["enabled"].as_bool() == Some(true) && !is_test(a))
        .filter_map(|a| Some((ts(&a["nextTimestamp"])?, a)))
        .filter(|(next, a)| ts(&a["endTimestamp"]).unwrap_or(*next) > now)
        .min_by_key(|(next, _)| *next)
        .map_or(Value::Null, |(_, a)| summary(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("8:30pm").unwrap(), "20:30:00");
        assert_eq!(parse_time("8:30 PM").unwrap(), "20:30:00");
        assert_eq!(parse_time("8pm").unwrap(), "20:00:00");
        assert_eq!(parse_time("830pm").unwrap(), "20:30:00");
        assert_eq!(parse_time("12am").unwrap(), "00:00:00");
        assert_eq!(parse_time("12:15pm").unwrap(), "12:15:00");
        assert_eq!(parse_time("7a").unwrap(), "07:00:00");
        assert_eq!(parse_time("20:30").unwrap(), "20:30:00");
        assert_eq!(parse_time("07:00").unwrap(), "07:00:00");
        assert_eq!(parse_time("0:05").unwrap(), "00:05:00");
        assert!(
            parse_time("8:30")
                .unwrap_err()
                .contains("morning or evening")
        );
        for bad in ["13pm", "25:00", "8:60pm", "8:3pm", "noon", ""] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_actions() {
        assert_eq!(parse_action("ON").unwrap(), Action::Enable(true));
        assert_eq!(parse_action("off").unwrap(), Action::Enable(false));
        assert_eq!(parse_action("9am").unwrap(), Action::Set("09:00:00".into()));
    }

    #[test]
    fn parses_days() {
        let d = parse_days("weekdays").unwrap();
        assert_eq!(d["friday"], true);
        assert_eq!(d["saturday"], false);
        let d = parse_days("mon, WED,fri").unwrap();
        assert_eq!(d["monday"], true);
        assert_eq!(d["tuesday"], false);
        assert_eq!(d["wednesday"], true);
        assert_eq!(parse_days("th").unwrap()["thursday"], true);
        assert!(parse_days("t").is_err());
        assert!(parse_days("funday").is_err());
        assert!(parse_days(",").is_err());
    }

    #[test]
    fn spots_test_alarms() {
        assert!(is_test(&json!({ "tags": [TEST_TAG] })));
        assert!(!is_test(&json!({ "tags": [] })));
        assert!(!is_test(&json!({})));
    }

    #[test]
    fn formats_clock() {
        assert_eq!(clock("09:15:00"), "9:15 AM");
        assert_eq!(clock("20:30:00"), "8:30 PM");
        assert_eq!(clock("00:05:00"), "12:05 AM");
        assert_eq!(clock("12:00:00"), "12:00 PM");
    }

    #[test]
    fn next_skips_disabled_and_finished() {
        let resp = json!({
            "alarms": [
                { "enabled": true, "nextTimestamp": "2000-01-01T07:00:00Z",
                  "endTimestamp": "2000-01-01T07:20:00Z" },
                { "enabled": false, "nextTimestamp": "2999-01-01T06:00:00Z" },
                { "enabled": true, "nextTimestamp": "2999-01-02T07:00:00Z",
                  "repeat": { "enabled": false } },
            ],
            "recommendedAlarm": { "enabled": false },
        });
        let a = next(&resp);
        assert_eq!(a["next"], "2999-01-02T07:00:00Z");
        assert_eq!(a["repeat"], false);
        assert_eq!(next(&json!({ "alarms": [] })), Value::Null);
    }
}
