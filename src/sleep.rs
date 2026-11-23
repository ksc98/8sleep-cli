//! `8sleep sleep`: nightly sleep reports from `GET client-api
//! /v1/users/{id}/trends`, whose `days[]` are keyed by wake date.

use owo_colors::OwoColorize;
use serde_json::{Value, json};

use crate::timeline::{self, Snore, Span, Stage};

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
    let avg = |v: Vec<f64>| -> Value {
        if v.is_empty() {
            Value::Null
        } else {
            json!(v.iter().sum::<f64>() / v.len() as f64)
        }
    };
    let low = |v: Vec<f64>| -> Value {
        v.into_iter()
            .reduce(f64::min)
            .map_or(Value::Null, |x| json!(x))
    };
    let lowest_at = day["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| {
            s["timeseries"]["heartRate"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .filter(|p| p[1].as_f64().is_some())
        .min_by(|a, b| a[1].as_f64().unwrap().total_cmp(&b[1].as_f64().unwrap()))
        .map_or(Value::Null, |p| p[0].clone());
    let efficiency = match (
        day["sleepDuration"].as_f64(),
        day["presenceDuration"].as_f64(),
    ) {
        (Some(s), Some(p)) if p > 0.0 => json!(s / p),
        _ => Value::Null,
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
        "efficiency": efficiency,
        // Plain statistics of the 5-minute series, comparable with other
        // trackers, beside Eight Sleep's own nightly figures.
        "heart_avg": avg(series("heartRate")),
        "heart_lowest": low(series("heartRate")),
        "heart_lowest_at": lowest_at,
        "rmssd_avg": avg(series("rmssd")),
        "heart_rate": q["heartRate"]["current"],
        "hrv": q["hrv"]["current"],
        "breathing_rate": q["respiratoryRate"]["current"],
        "snore_secs": day["snoreDuration"],
        "heavy_snore_secs": day["heavySnoreDuration"],
        "bed_c": range(series("tempBedC")),
        "room_c": range(series("tempRoomC")),
        // The night's stages and snoring as consecutive spans, with the time
        // out of bed between sessions as `out`.
        "timeline": stage_spans(day).iter().map(|s| json!({
            "start": iso(s.start), "end": iso(s.end), "stage": s.stage.name(),
        })).collect::<Vec<_>>(),
        "snore_spans": snore_spans(day).iter().map(|s| json!({
            "start": iso(s.start), "end": iso(s.end),
            "intensity": if s.heavy { "heavy" } else { "medium" },
        })).collect::<Vec<_>>(),
        "raw": day,
    })
}

/// The four times the app lets you edit on a session: got in bed, fell
/// asleep, woke up, got out of bed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    pub bed: jiff::Timestamp,
    pub asleep: jiff::Timestamp,
    pub wake: jiff::Timestamp,
    pub out: jiff::Timestamp,
}

/// The session to edit on a night: the main one, else the longest.
pub fn main_session(day: &Value) -> Option<&Value> {
    let list = day["sessions"].as_array()?;
    let main = day["mainSessionId"].as_str();
    list.iter()
        .find(|s| main.is_some() && s["id"].as_str() == main)
        .or_else(|| {
            list.iter()
                .max_by_key(|s| s["duration"].as_i64().unwrap_or(0))
        })
}

pub fn edges(session: &Value) -> anyhow::Result<Edges> {
    let at = |key: &str| -> anyhow::Result<jiff::Timestamp> {
        let s = session[key]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("session has no {key}"))?;
        s.parse()
            .map_err(|e| anyhow::anyhow!("session {key} {s:?}: {e}"))
    };
    Ok(Edges {
        bed: at("ts")?,
        asleep: at("sleepStart")?,
        wake: at("sleepEnd")?,
        out: at("presenceEnd")?,
    })
}

/// A clock time (`HH:MM:SS`, from `alarm::parse_time`) on whichever day in
/// `tz` puts it closest to `near`, so `--wake 11:08am` lands on the night's
/// own morning.
pub fn nearest(
    clock: &str,
    near: jiff::Timestamp,
    tz: &jiff::tz::TimeZone,
) -> anyhow::Result<jiff::Timestamp> {
    let time: jiff::civil::Time = clock.parse()?;
    let day = near.to_zoned(tz.clone()).date();
    let mut best: Option<jiff::Timestamp> = None;
    for offset in [-1, 0, 1] {
        let d = day.checked_add(jiff::Span::new().days(offset))?;
        let t = d.to_datetime(time).to_zoned(tz.clone())?.timestamp();
        let dist = |x: jiff::Timestamp| x.duration_since(near).abs();
        if best.is_none_or(|b| dist(t) < dist(b)) {
            best = Some(t);
        }
    }
    best.ok_or_else(|| anyhow::anyhow!("no time for {clock}"))
}

/// `PUT client-api /v1/users/{id}/intervals/{sessionId}`'s body, as the app
/// sends it: the in-bed start and three back-to-back runs (awake, asleep,
/// awake). The server rescores the stages inside the asleep run.
pub fn edit_body(e: &Edges, tz: &jiff::tz::TimeZone) -> anyhow::Result<Value> {
    if !(e.bed <= e.asleep && e.asleep < e.wake && e.wake <= e.out) {
        let at = |t: jiff::Timestamp| t.to_zoned(tz.clone()).strftime("%a %-I:%M %p").to_string();
        anyhow::bail!(
            "times out of order: in bed {}, asleep {}, awake {}, out of bed {}",
            at(e.bed),
            at(e.asleep),
            at(e.wake),
            at(e.out)
        );
    }
    let secs = |a: jiff::Timestamp, b: jiff::Timestamp| b.duration_since(a).as_secs();
    Ok(json!({
        "ts": e.bed.to_string(),
        "stages": [
            { "stage": "awake", "duration": secs(e.bed, e.asleep) },
            { "stage": "asleep", "duration": secs(e.asleep, e.wake) },
            { "stage": "awake", "duration": secs(e.wake, e.out) },
        ],
    }))
}

/// The sessions, oldest first, each with its start in unix seconds.
fn sessions(day: &Value) -> Vec<(i64, &Value)> {
    let mut v: Vec<(i64, &Value)> = day["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| Some((secs(&s["ts"])?, s)))
        .collect();
    v.sort_by_key(|s| s.0);
    v
}

/// A session's `{stage|intensity, duration}` list as back-to-back spans
/// from the session's start.
fn runs<'a>(
    start: i64,
    list: &'a Value,
    key: &'a str,
) -> impl Iterator<Item = (i64, i64, &'a str)> {
    let mut at = start;
    list.as_array().into_iter().flatten().map(move |r| {
        let from = at;
        at += r["duration"].as_i64().unwrap_or(0);
        (from, at, r[key].as_str().unwrap_or(""))
    })
}

/// Every session's stages, gaps between sessions as `out`, merged.
fn stage_spans(day: &Value) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for (start, s) in sessions(day) {
        if let Some(last) = spans.last()
            && start > last.end
        {
            spans.push(Span {
                start: last.end,
                end: start,
                stage: Stage::Out,
            });
        }
        for (a, b, stage) in runs(start, &s["stages"], "stage") {
            let Some(stage) = Stage::parse(stage) else {
                continue;
            };
            match spans.last_mut() {
                Some(l) if l.stage == stage && l.end == a => l.end = b,
                _ if b > a => spans.push(Span {
                    start: a,
                    end: b,
                    stage,
                }),
                _ => {}
            }
        }
    }
    spans
}

/// Every session's snoring (`medium` and `heavy`).
fn snore_spans(day: &Value) -> Vec<Snore> {
    sessions(day)
        .into_iter()
        .flat_map(|(start, s)| runs(start, &s["snoring"], "intensity"))
        .filter(|&(a, b, i)| b > a && matches!(i, "medium" | "heavy"))
        .map(|(start, end, i)| Snore {
            start,
            end,
            heavy: i == "heavy",
        })
        .collect()
}

/// The night for `timeline::render`, from a [`summary`].
fn night(n: &Value) -> timeline::Night {
    let series = |key: &str| timeline::Series {
        interval: 300,
        points: n["raw"]["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["timeseries"][key].as_array().into_iter().flatten())
            .filter_map(|p| Some((secs(&p[0])?, p[1].as_f64()?)))
            .collect(),
    };
    let list = |key: &str| n[key].as_array().cloned().unwrap_or_default();
    timeline::Night {
        in_bed: (
            secs(&n["in_bed"][0]).unwrap_or(0),
            secs(&n["in_bed"][1]).unwrap_or(0),
        ),
        stages: list("timeline")
            .iter()
            .filter_map(|s| {
                Some(Span {
                    start: secs(&s["start"])?,
                    end: secs(&s["end"])?,
                    stage: Stage::parse(s["stage"].as_str()?)?,
                })
            })
            .collect(),
        snoring: list("snore_spans")
            .iter()
            .filter_map(|s| {
                Some(Snore {
                    start: secs(&s["start"])?,
                    end: secs(&s["end"])?,
                    heavy: s["intensity"] == "heavy",
                })
            })
            .collect(),
        heart: series("heartRate"),
        // RMSSD, the HRV row's figure and the one Oura reports.
        hrv: series("rmssd"),
        lowest: secs(&n["heart_lowest_at"]),
    }
}

/// A night as text lines, with its timeline drawn for `style`.
pub fn lines(n: &Value, style: &timeline::Style) -> Vec<String> {
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
    let row = |out: &mut Vec<String>, label: &str, value: String| {
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
        n["efficiency"]
            .as_f64()
            .map(|e| format!("efficiency {:.0}%", e * 100.0)),
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
    row(&mut out, "sleep", sleep);

    let sc = &n["score"];
    row(
        &mut out,
        "scores",
        join([
            num(&sc["fitness"]).map(|v| format!("fitness {}", v.bold())),
            num(&sc["quality"]).map(|v| format!("quality {v}")),
            num(&sc["routine"]).map(|v| format!("routine {v}")),
        ]),
    );
    row(
        &mut out,
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
    let tz = jiff::tz::TimeZone::system();
    let offset =
        |t: i64| jiff::Timestamp::from_second(t).map_or(0, |ts| tz.to_offset(ts).seconds() as i64);
    out.extend(timeline::render(&night(n), &offset, style));
    row(
        &mut out,
        "heart",
        join([
            n["heart_avg"].as_f64().map(|v| format!("avg {v:.0}")),
            num(&n["heart_lowest"]).map(|v| match clock(&n["heart_lowest_at"]) {
                Some(at) => format!("lowest {v} bpm at {at}"),
                None => format!("lowest {v} bpm"),
            }),
            num(&n["heart_rate"]).map(|v| format!("{}", format!("Eight Sleep {v}").dimmed())),
        ]),
    );
    row(
        &mut out,
        "HRV",
        join([
            n["rmssd_avg"].as_f64().map(|v| format!("avg {v:.0} ms")),
            num(&n["hrv"]).map(|v| format!("{}", format!("Eight Sleep {v}").dimmed())),
        ]),
    );
    row(
        &mut out,
        "breathing",
        num(&n["breathing_rate"]).map_or(String::new(), |v| format!("{v} /min")),
    );
    let snore = dur(&n["snore_secs"]).map(|d| match dur(&n["heavy_snore_secs"]) {
        Some(h) => format!("{d} {}", format!("(heavy {h})").dimmed()),
        None => d,
    });
    row(&mut out, "snoring", snore.unwrap_or_else(|| "none".into()));
    row(
        &mut out,
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

fn secs(v: &Value) -> Option<i64> {
    Some(v.as_str()?.parse::<jiff::Timestamp>().ok()?.as_second())
}

fn iso(secs: i64) -> String {
    jiff::Timestamp::from_second(secs).map_or(String::new(), |t| t.to_string())
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
    fn edits_a_session() {
        let session = json!({
            "ts": "2026-11-22T10:12:00.000Z",
            "sleepStart": "2026-11-22T10:54:00.000Z",
            "sleepEnd": "2026-11-22T18:30:00.000Z",
            "presenceEnd": "2026-11-22T18:31:00.000Z",
        });
        let mut e = edges(&session).unwrap();
        let la = jiff::tz::TimeZone::get("America/Los_Angeles").unwrap();
        // 11:08 AM lands on the night's morning, not the day before or after.
        e.wake = nearest("11:08:00", e.wake, &la).unwrap();
        e.out = nearest("11:09:00", e.out, &la).unwrap();
        assert_eq!(e.wake.to_string(), "2026-11-22T19:08:00Z");
        // 11 PM, near 2:12 AM, is the evening before.
        assert_eq!(
            nearest("23:00:00", e.bed, &la).unwrap().to_string(),
            "2026-11-22T07:00:00Z"
        );
        assert_eq!(
            edit_body(&e, &la).unwrap(),
            json!({
                "ts": "2026-11-22T10:12:00Z",
                "stages": [
                    { "stage": "awake", "duration": 2520 },
                    { "stage": "asleep", "duration": 29640 },
                    { "stage": "awake", "duration": 60 },
                ],
            })
        );
        e.out = e.asleep;
        assert!(edit_body(&e, &la).is_err());
    }

    #[test]
    fn formats_durations() {
        assert_eq!(dur(&json!(20010)).unwrap(), "5h 34m");
        assert_eq!(dur(&json!(660)).unwrap(), "11m");
        assert_eq!(dur(&json!(3599)).unwrap(), "1h 0m");
        assert!(dur(&json!(0)).is_none());
        assert!(dur(&Value::Null).is_none());
    }

    #[test]
    fn spans_sessions_with_time_out_of_bed_between() {
        let day = json!({ "sessions": [
            // Out of order on purpose: spans follow the sessions' starts.
            { "ts": "2026-11-20T13:00:00Z",
              "stages": [{ "stage": "light", "duration": 600 }, { "stage": "deep", "duration": 600 }],
              "snoring": [{ "intensity": "none", "duration": 300 }, { "intensity": "heavy", "duration": 60 }] },
            { "ts": "2026-11-20T11:00:00Z",
              "stages": [{ "stage": "awake", "duration": 300 }, { "stage": "light", "duration": 3300 },
                         { "stage": "out", "duration": 600 }, { "stage": "light", "duration": 300 },
                         { "stage": "mystery", "duration": 60 }],
              "snoring": [{ "intensity": "medium", "duration": 120 }] },
        ]});
        let n = summary(&day);
        let stages: Vec<(&str, &str, &str)> = n["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["start"].as_str().unwrap(),
                    s["end"].as_str().unwrap(),
                    s["stage"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            stages,
            [
                ("2026-11-20T11:00:00Z", "2026-11-20T11:05:00Z", "awake"),
                ("2026-11-20T11:05:00Z", "2026-11-20T12:00:00Z", "light"),
                ("2026-11-20T12:00:00Z", "2026-11-20T12:10:00Z", "out"),
                ("2026-11-20T12:10:00Z", "2026-11-20T12:15:00Z", "light"),
                // The unknown stage's minute and the gap to 13:00 are out of bed.
                ("2026-11-20T12:15:00Z", "2026-11-20T13:00:00Z", "out"),
                ("2026-11-20T13:00:00Z", "2026-11-20T13:10:00Z", "light"),
                ("2026-11-20T13:10:00Z", "2026-11-20T13:20:00Z", "deep"),
            ]
        );
        assert_eq!(
            n["snore_spans"],
            json!([
                { "start": "2026-11-20T11:00:00Z", "end": "2026-11-20T11:02:00Z", "intensity": "medium" },
                { "start": "2026-11-20T13:05:00Z", "end": "2026-11-20T13:06:00Z", "intensity": "heavy" },
            ])
        );
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
        let style = timeline::Style {
            width: 100,
            color: false,
            legend: true,
        };
        let text = lines(&n, &style).join("\n");
        assert!(text.contains("Eight Sleep 44.6"), "{text}");
        assert!(text.contains("79–85°F"), "{text}");
        assert!(text.contains("none"), "{text}");
        assert_eq!(
            nights(&json!({ "days": [day, { "day": "2026-11-21", "sleepDuration": 0 }] })).len(),
            1
        );
    }
}
