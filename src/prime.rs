//! `8sleep prime`: the pod's priming state, schedule and recent primes, from
//! `app-api /v1/devices/{id}/status`, `.../priming/tasks` and
//! `.../priming/schedule`.

use owo_colors::OwoColorize;
use serde_json::{Value, json};

use crate::{alarm, ui};

/// How many past primes the human view lists.
const HISTORY: usize = 5;

/// The view `--json` prints and the renderer reads: the live priming state,
/// whether a prime is running (the newest task hasn't finished), the
/// schedule as given (pod-local `Monday 14:00`) and the task history,
/// newest first.
pub fn summary(status: &Value, tasks: &Value, schedule: &Value) -> Value {
    let tasks: Vec<Value> = tasks["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(task)
        .collect();
    let running = tasks.first().is_some_and(|t| t["status"] != "finished");
    json!({
        "running": running,
        "water": water(status),
        "ready": status["ready"],
        "priming": status["priming"],
        "schedule": schedule["schedule"],
        "tasks": tasks,
    })
}

/// One prime in the shape `--json` and the renderer share.
fn task(t: &Value) -> Value {
    let secs = match (ts(&t["started"]), ts(&t["finished"])) {
        (Some(a), Some(b)) => Some((b - a).get_seconds()),
        _ => None,
    };
    let end = &t["primingStateAtCompletion"];
    json!({
        "reason": t["reason"],
        "meta": t["notifications"]["meta"],
        "created": t["created"],
        "started": t["started"],
        "finished": t["finished"],
        "status": t["status"],
        "outcome": t["outcome"],
        "outcome_reason": t["outcomeReason"],
        "seconds": secs,
        "errors": end["errors"],
        "end_state": end,
    })
}

fn ts(v: &Value) -> Option<jiff::Timestamp> {
    v.as_str()?.parse().ok()
}

/// The same words as `status`'s header.
fn water(status: &Value) -> &'static str {
    if status["needsPrime"].as_bool() == Some(true) {
        "needs priming"
    } else if status["hasWater"].as_bool() == Some(false) {
        "no water"
    } else {
        "water ok"
    }
}

pub fn lines(s: &Value) -> Vec<String> {
    let sep = " · ".dimmed().to_string();
    let mut out = Vec::new();

    let state = if s["running"].as_bool() == Some(true) {
        let mut line = format!("{} {}", "●".cyan(), "priming".cyan().bold());
        if let Some(p) = progress(&s["priming"]) {
            line.push_str(&format!("{sep}{p}"));
        }
        if let Some(t) = s["tasks"].get(0) {
            let since = t["started"].as_str().or(t["created"].as_str());
            if let Some(w) = ui::when(since) {
                line.push_str(&format!("{sep}{}", format!("since {w}").dimmed()));
            }
        }
        line
    } else {
        match s["water"].as_str().unwrap_or("?") {
            w @ "water ok" => format!("{} {}", "●".green(), w.green()),
            w => format!("{} {}", "●".yellow(), w.yellow().bold()),
        }
    };
    out.push(ui::row("", "state", state));
    out.push(ui::row("", "schedule", schedule_summary(&s["schedule"])));

    let tasks = s["tasks"].as_array().map(Vec::as_slice).unwrap_or_default();
    if tasks.is_empty() {
        out.push(ui::row("", "last", "never".dimmed()));
        return out;
    }
    out.push(String::new());
    for t in tasks.iter().take(HISTORY) {
        out.push(task_line(t));
    }
    out
}

/// `flush 40%`, from the live `/status` priming block. Only `idle` and
/// `complete` states have been seen, so print whatever is there.
fn progress(p: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(state) = p["state"].as_str() {
        parts.push(state.to_string());
    }
    if let Some(phase) = p["phase"].as_str() {
        parts.push(phase.to_string());
    }
    if let Some(pct) = p["flushProgress"].as_f64() {
        parts.push(format!("{pct:.0}%"));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// `    today 8:02 PM  requested (fill)   3m 3s  ✓ succeeded`.
fn task_line(t: &Value) -> String {
    let at = t["started"].as_str().or(t["created"].as_str());
    let at = ui::when(at).unwrap_or_else(|| "?".into());
    let reason = match (t["reason"].as_str(), t["meta"].as_str()) {
        (Some("scheduled"), _) => "scheduled".to_string(),
        (Some(r), Some("fill_pod")) => format!("{r} (fill)"),
        (Some(r), _) => r.to_string(),
        (None, _) => "?".to_string(),
    };
    let outcome = if t["status"] != "finished" {
        format!("{} {}", "●".cyan(), "running".cyan())
    } else {
        match t["outcome"].as_str() {
            Some("succeeded") => format!("{} {}", "✓".green(), "succeeded".green()),
            Some(o) => {
                let mut s = format!("{} {}", "✗".red(), o.red().bold());
                let why = errors(t).or_else(|| {
                    t["outcome_reason"]
                        .as_str()
                        .filter(|r| *r != o)
                        .map(str::to_string)
                });
                if let Some(why) = why {
                    s.push_str(&format!(" {}", format!("({why})").dimmed()));
                }
                s
            }
            None => "?".to_string(),
        }
    };
    let took = t["seconds"].as_i64().map(duration).unwrap_or_default();
    let took = format!("{took:>7}").dimmed().to_string();
    let at = format!("{at:>17}").dimmed().to_string();
    format!("  {at}  {reason:<16}  {took}  {outcome}")
}

fn errors(t: &Value) -> Option<String> {
    let list: Vec<&str> = t["errors"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    (!list.is_empty()).then(|| list.join(", "))
}

/// `3m 3s`, `45s`.
fn duration(secs: i64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s}s"),
        (m, s) => format!("{m}m {s}s"),
    }
}

/// `daily 2:00 PM`, or `Mon Wed 2:00 PM, Sat 9:00 AM`. The entries are the
/// pod's local time (`Monday 14:00`) and are shown as given.
fn schedule_summary(list: &Value) -> String {
    const DAYS: [&str; 7] = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    // Group days by time, in time order.
    let mut by_time: std::collections::BTreeMap<(u32, u32), [bool; 7]> = Default::default();
    let mut odd = Vec::new();
    for e in list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let parsed = e.split_once(' ').and_then(|(day, time)| {
            let d = DAYS.iter().position(|k| k.eq_ignore_ascii_case(day))?;
            let (h, m) = time.split_once(':')?;
            Some((d, (h.parse().ok()?, m.parse().ok()?)))
        });
        match parsed {
            Some((d, hm)) => by_time.entry(hm).or_default()[d] = true,
            None => odd.push(e.to_string()),
        }
    }
    if by_time.is_empty() && odd.is_empty() {
        return "none".dimmed().to_string();
    }
    let mut parts: Vec<String> = by_time
        .iter()
        .map(|((h, m), on)| {
            let days: serde_json::Map<String, Value> = DAYS
                .iter()
                .zip(on)
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect();
            format!(
                "{} {}",
                ui::days_summary(&Value::Object(days)),
                alarm::clock(&format!("{h}:{m:02}")).bold()
            )
        })
        .collect();
    parts.extend(odd);
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, 'm') => esc = false,
                (true, _) => {}
                (false, c) => out.push(c),
            }
        }
        out
    }

    fn running() -> Value {
        json!({"tasks": [
            {"created": "2026-11-24T04:02:23Z", "started": "2026-11-24T04:02:35Z",
             "reason": "requested", "status": "started",
             "notifications": {"meta": "fill_pod", "users": ["u"]}},
            {"created": "2026-11-23T22:00:06Z", "started": "2026-11-23T22:00:46Z",
             "finished": "2026-11-23T22:03:35Z", "reason": "scheduled", "status": "finished",
             "outcome": "succeeded", "outcomeReason": "complete",
             "primingStateAtCompletion": {"flushProgress": 100, "phase": "flush", "state": "complete",
                                          "waterLevelLeft": 50, "waterLevelRight": 50}},
            {"created": "2026-11-21T23:33:36Z", "started": null, "finished": "2026-11-21T23:38:42Z",
             "reason": "requested", "status": "finished", "outcome": "expired",
             "notifications": {"meta": "rePriming"}},
            {"created": "2026-11-20T23:00:00Z", "started": "2026-11-20T23:00:10Z",
             "finished": "2026-11-20T23:01:00Z", "reason": "requested", "status": "finished",
             "outcome": "failed", "primingStateAtCompletion": {"errors": ["ucDisconnect"]}}
        ]})
    }

    #[test]
    fn running_task_shows_live_progress() {
        let status = json!({"hasWater": true, "needsPrime": false, "ready": true,
            "priming": {"state": "priming", "phase": "flush", "flushProgress": 40}});
        let s = summary(&status, &running(), &json!({"schedule": []}));
        assert_eq!(s["running"], true);
        assert_eq!(s["tasks"][0]["seconds"], Value::Null);
        assert_eq!(s["tasks"][1]["seconds"], 169);
        let out: Vec<String> = lines(&s).iter().map(|l| plain(l)).collect();
        assert!(
            out[0].contains("priming · priming flush 40% · since "),
            "{}",
            out[0]
        );
        assert!(out[3].contains("requested (fill)") && out[3].contains("running"));
        assert!(out[4].contains("scheduled") && out[4].contains("2m 49s  ✓ succeeded"));
        assert!(
            out[5].contains("✗ expired") && !out[5].contains('('),
            "{}",
            out[5]
        );
        assert!(out[6].contains("✗ failed (ucDisconnect)"), "{}", out[6]);
    }

    #[test]
    fn idle_shows_water() {
        let status = json!({"hasWater": true, "needsPrime": true});
        let s = summary(&status, &json!({"tasks": []}), &json!({}));
        assert_eq!(s["running"], false);
        let out: Vec<String> = lines(&s).iter().map(|l| plain(l)).collect();
        assert!(out[0].ends_with("● needs priming"));
        assert!(out[1].ends_with("none"));
        assert!(out[2].ends_with("never"));
    }

    #[test]
    fn summarizes_schedule() {
        let daily: Vec<String> = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ]
        .iter()
        .map(|d| format!("{d} 14:00"))
        .collect();
        assert_eq!(plain(&schedule_summary(&json!(daily))), "daily 2:00 PM");
        let mixed = json!([
            "Saturday 09:30",
            "Monday 14:00",
            "Wednesday 14:00",
            "whenever"
        ]);
        assert_eq!(
            plain(&schedule_summary(&mixed)),
            "Sat 9:30 AM, Mon Wed 2:00 PM, whenever"
        );
    }

    #[test]
    fn formats_duration() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(183), "3m 3s");
    }
}
