//! `8sleep sleep`: nightly sleep reports from `GET client-api
//! /v1/users/{id}/trends`, whose `days[]` are keyed by wake date.

use owo_colors::OwoColorize;
use serde_json::{Value, json};

/// The trends for wake dates `from..=to`, with every session.
pub fn url(user: &str, from: jiff::civil::Date, to: jiff::civil::Date) -> String {
    let tz = jiff::tz::TimeZone::system();
    let tz = tz.iana_name().unwrap_or("UTC");
    format!(
        "{}/users/{user}/trends?tz={tz}&from={from}&to={to}&include-main=false&include-all-sessions=true&model-version=v3&consistent-read=false",
        crate::api::CLIENT_API,
    )
}

/// The nights in a trends response that recorded sleep, newest first.
pub fn nights(trends: &Value) -> Vec<&Value> {
    let mut days: Vec<&Value> = trends["days"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["sleepDuration"].as_i64().unwrap_or(0) > 0)
        .collect();
    days.sort_by(|a, b| b["day"].as_str().cmp(&a["day"].as_str()));
    days
}

/// One night's headline numbers, in the shape `--json` prints (with the
/// API's whole day under `raw`).
pub fn summary(day: &Value) -> Value {
    let q = &day["sleepQualityScore"];
    let r = &day["sleepRoutineScore"];
    let s = &day["sessions"][0]["stageSummary"];
    let series = |key: &str| -> Vec<f64> {
        day["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["timeseries"][key].as_array().into_iter().flatten())
            .filter_map(|p| p[1].as_f64())
            .collect()
    };
    let range = |v: Vec<f64>| -> Value {
        let lo = v.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if v.is_empty() {
            Value::Null
        } else {
            json!([lo, hi])
        }
    };
    json!({
        "day": day["day"],
        "in_bed": [day["presenceStart"], day["presenceEnd"]],
        "asleep": [day["sleepStart"], day["sleepEnd"]],
        "in_bed_secs": day["presenceDuration"],
        "sleep_secs": day["sleepDuration"],
        "light_secs": day["lightDuration"],
        "deep_secs": day["deepDuration"],
        "rem_secs": day["remDuration"],
        "light_pct": s["lightPercentOfSleep"],
        "deep_pct": day["deepPercent"],
        "rem_pct": day["remPercent"],
        "latency_secs": r["latencyAsleepSeconds"]["current"],
        "awake_secs": day["wasoDuration"],
        "wakeups": day["wakeupCount"],
        "score": { "fitness": day["score"], "quality": q["total"], "routine": r["total"] },
        "heart_rate": q["heartRate"]["current"],
        "hrv": q["hrv"]["current"],
        "breathing_rate": q["respiratoryRate"]["current"],
        "snore_secs": day["snoreDuration"],
        "heavy_snore_secs": day["heavySnoreDuration"],
        "bed_c": range(series("tempBedC")),
        "room_c": range(series("tempRoomC")),
        "raw": day,
    })
}

/// A night as text lines.
pub fn lines(n: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let date = n["day"]
        .as_str()
        .and_then(|d| d.parse::<jiff::civil::Date>().ok())
        .map_or("?".into(), |d| d.strftime("%a %b %-d").to_string());
    let span = |pair: &Value| match (clock(&pair[0]), clock(&pair[1])) {
        (Some(a), Some(b)) => format!("{a} – {b}"),
        _ => "?".into(),
    };
    out.push(format!(
        "{}  {}",
        date.bold(),
        format!(
            "asleep {} · in bed {}",
            span(&n["asleep"]),
            span(&n["in_bed"])
        )
        .dimmed()
    ));
    let mut row = |label: &str, value: String| {
        if !value.is_empty() {
            out.push(format!("  {:<10} {value}", label.dimmed()));
        }
    };

    let mut sleep = dur(&n["sleep_secs"]).unwrap_or_default();
    let extras: Vec<String> = [
        dur(&n["latency_secs"]).map(|d| format!("fell asleep in {d}")),
        dur(&n["awake_secs"]).map(|d| {
            let w = n["wakeups"].as_i64().unwrap_or(0);
            format!("awake {d}, {w} wakeup{}", if w == 1 { "" } else { "s" })
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !extras.is_empty() {
        sleep = format!(
            "{} {}",
            sleep.bold(),
            format!("· {}", extras.join(" · ")).dimmed()
        );
    }
    row("sleep", sleep);

    let sc = &n["score"];
    row(
        "scores",
        join([
            num(&sc["fitness"]).map(|v| format!("fitness {}", v.bold())),
            num(&sc["quality"]).map(|v| format!("quality {v}")),
            num(&sc["routine"]).map(|v| format!("routine {v}")),
        ]),
    );
    row(
        "stages",
        join(["light", "deep", "rem"].map(|k| {
            let d = dur(&n[format!("{k}_secs")])?;
            let label = if k == "rem" { "REM" } else { k };
            Some(match n[format!("{k}_pct")].as_f64() {
                Some(p) => format!("{label} {d} {}", format!("{:.0}%", p * 100.0).dimmed()),
                None => format!("{label} {d}"),
            })
        })),
    );
    row(
        "heart",
        join([
            num(&n["heart_rate"]).map(|v| format!("{v} bpm")),
            num(&n["hrv"]).map(|v| format!("HRV {v} ms")),
        ]),
    );
    row(
        "breathing",
        num(&n["breathing_rate"]).map_or(String::new(), |v| format!("{v} /min")),
    );
    let snore = dur(&n["snore_secs"]).map(|d| match dur(&n["heavy_snore_secs"]) {
        Some(h) => format!("{d} {}", format!("(heavy {h})").dimmed()),
        None => d,
    });
    row("snoring", snore.unwrap_or_else(|| "none".into()));
    row(
        "temp",
        join([
            f_range(&n["bed_c"]).map(|r| format!("bed {r}")),
            f_range(&n["room_c"]).map(|r| format!("room {r}")),
        ]),
    );
    out
}

fn join<const N: usize>(parts: [Option<String>; N]) -> String {
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// A number with at most one decimal.
fn num(v: &Value) -> Option<String> {
    let f = v.as_f64()?;
    Some(if f.fract() == 0.0 {
        format!("{f:.0}")
    } else {
        format!("{f:.1}")
    })
}

/// `20010` seconds -> `5h 33m`; zero and missing -> `None`.
fn dur(v: &Value) -> Option<String> {
    let secs = v.as_f64()? as i64;
    if secs <= 0 {
        return None;
    }
    let (h, m) = (secs / 3600, (secs % 3600 + 30) / 60);
    let (h, m) = if m == 60 { (h + 1, 0) } else { (h, m) };
    Some(match h {
        0 => format!("{m}m"),
        _ => format!("{h}h {m}m"),
    })
}

/// A UTC timestamp as a local clock time, `3:02 AM`.
fn clock(v: &Value) -> Option<String> {
    let t: jiff::Timestamp = v.as_str()?.parse().ok()?;
    Some(
        t.to_zoned(jiff::tz::TimeZone::system())
            .strftime("%-I:%M %p")
            .to_string(),
    )
}

/// `[lo, hi]` in °C as a °F range, `79–85°F`.
fn f_range(v: &Value) -> Option<String> {
    let f = |c: f64| (c * 9.0 / 5.0 + 32.0).round() as i64;
    let (lo, hi) = (f(v[0].as_f64()?), f(v[1].as_f64()?));
    Some(if lo == hi {
        format!("{lo}°F")
    } else {
        format!("{lo}–{hi}°F")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(dur(&json!(20010)).unwrap(), "5h 34m");
        assert_eq!(dur(&json!(660)).unwrap(), "11m");
        assert_eq!(dur(&json!(3599)).unwrap(), "1h 0m");
        assert!(dur(&json!(0)).is_none());
        assert!(dur(&Value::Null).is_none());
    }

    #[test]
    fn summarizes_a_night() {
        let day = json!({
            "day": "2026-11-20", "sleepDuration": 20010, "deepDuration": 4320, "deepPercent": 0.22,
            "wasoDuration": 660, "wakeupCount": 1, "score": 68, "snoreDuration": 0,
            "sleepQualityScore": { "total": 84, "heartRate": { "current": 44.6 } },
            "sessions": [{ "timeseries": { "tempBedC": [["t", 26.0], ["t", 29.5]] } }],
        });
        let n = summary(&day);
        assert_eq!(n["bed_c"], json!([26.0, 29.5]));
        assert_eq!(n["room_c"], Value::Null);
        let text = lines(&n).join("\n");
        assert!(text.contains("44.6 bpm"), "{text}");
        assert!(text.contains("79–85°F"), "{text}");
        assert!(text.contains("none"), "{text}");
        assert_eq!(
            nights(&json!({ "days": [day, { "day": "2026-11-21", "sleepDuration": 0 }] })).len(),
            1
        );
    }
}
