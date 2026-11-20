//! Terminal rendering. Everything is printed through `anstream`, which strips
//! the ANSI codes when stdout isn't a terminal or `NO_COLOR` is set.

use clap::builder::styling::{AnsiColor, Effects, Styles};
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::{alarm, temp};

const LABEL_WIDTH: usize = 9;
/// Cargo's help colors.
pub const HELP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::BrightRed.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .invalid(AnsiColor::Yellow.on_default().effects(Effects::BOLD));

const GAUGE_CELLS: i32 = 41; // one cell per half step of the app's -10..=+10 scale
const TARGET_MARK: &str = "◆";
const NOW_MARK: &str = "●";

const COOL: (f64, f64, f64) = (80.0, 150.0, 255.0);
const NEUTRAL: (f64, f64, f64) = (200.0, 200.0, 200.0);
const WARM: (f64, f64, f64) = (255.0, 135.0, 60.0);

/// Blue for cooling, grey at neutral, orange for warming.
fn level_rgb(raw: i32) -> (u8, u8, u8) {
    let t = (raw.clamp(-100, 100) as f64) / 100.0;
    let (from, to, k) = if t < 0.0 {
        (NEUTRAL, COOL, -t)
    } else {
        (NEUTRAL, WARM, t)
    };
    let mix = |a: f64, b: f64| (a + (b - a) * k).round() as u8;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

fn colored_level(text: &str, raw: i32) -> String {
    let (r, g, b) = level_rgb(raw);
    text.truecolor(r, g, b).to_string()
}

/// `68°F  -6`, the degrees colored by level and the app scale dimmed.
/// `111°F  +10` / ` 86°F  +1.7`: degrees right-aligned to the widest
/// value (111°F) and the app level left-aligned behind a sign column, so
/// stacked rows line up. Padding happens before coloring because escape
/// codes would count toward the width.
fn temp_pair(raw: i32, bold: bool, precise: bool) -> String {
    let f = format!("{:>5}", format!("{}°F", temp::raw_to_f(raw)));
    let f = if bold {
        colored_level(&f, raw).bold().to_string()
    } else {
        colored_level(&f, raw)
    };
    let app = if precise {
        temp::raw_to_app_precise(raw)
    } else {
        temp::raw_to_app(raw)
    };
    // "0" has no sign; give it the sign column so digits stay aligned.
    let app = if app.starts_with(['+', '-']) {
        app
    } else {
        format!(" {app}")
    };
    format!("{f}  {}", app.dimmed())
}

fn gauge_cell(raw: i32) -> i32 {
    ((raw.clamp(-100, 100) + 100) as f64 * (GAUGE_CELLS - 1) as f64 / 200.0).round() as i32
}

/// A cool→warm bar marking the target (◆) and, when known, the current
/// level (●). While the bed is on, the stretch between them is drawn heavy to
/// show which way it's heading; when it's off nothing is heading anywhere.
fn gauge(target: i32, now: Option<i32>, on: bool) -> String {
    let t = gauge_cell(target);
    let n = now.map(gauge_cell);
    let (lo, hi) = match n {
        Some(n) if on => (n.min(t), n.max(t)),
        _ => (t, t),
    };
    let bar: String = (0..GAUGE_CELLS)
        .map(|i| {
            let (r, g, b) = level_rgb(i * 200 / (GAUGE_CELLS - 1) - 100);
            if i == t {
                TARGET_MARK.bright_white().bold().to_string()
            } else if Some(i) == n {
                NOW_MARK.bright_white().to_string()
            } else if i > lo && i < hi {
                "━".truecolor(r, g, b).bold().to_string()
            } else {
                "─".truecolor(r, g, b).to_string()
            }
        })
        .collect();
    format!("{} {bar} {}", "cool".dimmed(), "warm".dimmed())
}

/// `  ◆ target   81°F`: an optional bar marker, a dimmed label, the value.
fn row(mark: &str, label: &str, value: impl std::fmt::Display) -> String {
    let mark = if mark.is_empty() {
        " ".to_string()
    } else {
        mark.bright_white().to_string()
    };
    format!(
        "  {mark} {}{value}",
        format!("{label:<LABEL_WIDTH$}").dimmed()
    )
}

/// `today 3:33 PM`, `tomorrow 2:15 AM`, `Fri 2:15 AM` or `Nov 12 3:33 PM`.
pub fn when(ts: Option<&str>) -> Option<String> {
    let t: jiff::Timestamp = ts?.parse().ok()?;
    let z = t.to_zoned(jiff::tz::TimeZone::system());
    let today = jiff::Zoned::now().date();
    let days = (z.date() - today).get_days();
    let time = z.strftime("%-I:%M %p");
    Some(match days {
        0 => format!("today {time}"),
        1 => format!("tomorrow {time}"),
        -1 => format!("yesterday {time}"),
        2..=6 => z.strftime("%a %-I:%M %p").to_string(),
        _ => z.strftime("%b %-d %-I:%M %p").to_string(),
    })
}

/// Header line: model, connectivity, water.
pub fn device_line(dev: &Value, water: &str) -> String {
    let model = dev["modelString"].as_str().unwrap_or("Pod");
    let online = if dev["online"].as_bool() == Some(true) {
        format!("{} online", "●".green())
    } else {
        format!("{} {}", "●".red(), "offline".red().bold())
    };
    let water = match water {
        "water ok" => water.green().to_string(),
        "priming" => water.cyan().to_string(),
        _ => water.yellow().bold().to_string(),
    };
    let sep = " · ".dimmed().to_string();
    format!("{}{sep}{online}{sep}{water}", model.bold())
}

/// The block for one side of the bed (or the whole bed in solo mode).
pub fn side_block(title: &str, you: bool, s: &Value) -> Vec<String> {
    let mut head = format!("  {}", title.bold());
    if you {
        head.push_str(&format!(" {}", "(you)".dimmed()));
    }
    if s["state"] == "unassigned" {
        head.push_str(&format!("  {}", "unassigned".dimmed()));
        return vec![head];
    }
    let on = s["on"].as_bool() == Some(true);
    let power = if on {
        format!("{} {}", "●".green(), "on".green().bold())
    } else {
        format!("{} {}", "○".dimmed(), "off".dimmed())
    };
    head.push_str(&format!("   {power}"));
    // Smart phases show up in the schedule row; anything else is unusual.
    let state = s["state"].as_str().unwrap_or("?");
    if on && !state.starts_with("smart") {
        head.push_str(&format!("   {}", state.dimmed()));
    }

    let mut lines = vec![head];
    let target = s["target"].as_i64().map(|v| v as i32);
    // The API keeps reporting the current level while the bed is off.
    let now = s["level"].as_i64().map(|v| v as i32);
    if let Some(t) = target {
        lines.push(row(TARGET_MARK, "target", temp_pair(t, true, false)));
    }
    if let Some(n) = now {
        lines.push(row(NOW_MARK, "current", temp_pair(n, false, true)));
    }
    if let Some(t) = target {
        lines.push(row("", "", gauge(t, now, on)));
    }
    lines.extend(phase_rows(s));
    if let Some(alarm) = alarm_row(&s["alarm"]) {
        lines.push(alarm);
    }
    if s["away"].as_bool() == Some(true) {
        lines.push(row("", "away", "on".yellow().bold()));
    }
    lines
}

/// The smart schedule on one line: `now │ night │ dawn` (the running phase
/// marked), then when it next starts and whether Autopilot is adjusting it.
fn phase_rows(s: &Value) -> Vec<String> {
    let (cells, mut notes) = phase_cells(&s["phases"]);
    if cells.is_empty() {
        return Vec::new();
    }
    if let Some(time) = s["bedtime"]["time"].as_str() {
        notes.push(format!(
            "bedtime {} {}",
            alarm::clock(time),
            days_summary(&s["bedtime"]["days"])
        ));
    }
    if s["autopilot"].as_bool() == Some(true) {
        notes.push("Autopilot".to_string());
    }
    vec![row("", "schedule", with_notes(cells, &notes))]
}

/// `now 81°F 0 │ night 78°F -1 │ dawn 82°F 0`, plus notes for phases changed
/// for tonight.
fn phase_cells(phases: &Value) -> (String, Vec<String>) {
    let mut cells = Vec::new();
    let mut notes = Vec::new();
    for p in phases.as_array().into_iter().flatten() {
        let Some(level) = p["level"].as_i64().map(|v| v as i32) else {
            continue;
        };
        let name = phase_name(p["phase"].as_str().unwrap_or("?"));
        let active = p["active"].as_bool() == Some(true);
        let label = if active {
            format!("{} {}", "▸".bright_white().bold(), name.bold())
        } else {
            name.dimmed().to_string()
        };
        let f = colored_level(&format!("{}°F", temp::raw_to_f(level)), level);
        cells.push(format!("{label} {f} {}", temp::raw_to_app(level).dimmed()));
        if p["overridden"].as_bool() == Some(true)
            && let Some(sched) = p["scheduled"].as_i64()
        {
            notes.push(format!(
                "{name} changed tonight (schedule {})",
                temp::raw_to_app(sched as i32)
            ));
        }
    }
    (cells.join(&" │ ".dimmed().to_string()), notes)
}

fn with_notes(mut line: String, notes: &[String]) -> String {
    if !notes.is_empty() {
        line.push_str(&format!("   {}", notes.join(" · ").dimmed()));
    }
    line
}

/// `8sleep schedule`: when it turns the bed on, then the level for each phase.
pub fn schedule_block(bedtimes: &[Value], phases: &Value, autopilot: bool) -> Vec<String> {
    let mut out = Vec::new();
    if bedtimes.is_empty() {
        out.push(row("", "bedtime", "none".dimmed()));
    }
    for (i, b) in bedtimes.iter().enumerate() {
        let on = b["enabled"].as_bool() == Some(true);
        let power = if on {
            format!("{} {}", "●".green(), "on ".green().bold())
        } else {
            format!("{} {}", "○".dimmed(), "off".dimmed())
        };
        // Pad before coloring: escape codes would count toward the width.
        let time = format!("{:>8}", alarm::clock(b["time"].as_str().unwrap_or("?")));
        let time = if on {
            time.bold().to_string()
        } else {
            time.dimmed().to_string()
        };
        let label = if i == 0 { "bedtime" } else { "" };
        out.push(row(
            "",
            label,
            format!(
                "{}  {power}  {time}  {}",
                (i + 1).dimmed(),
                days_summary(&b["days"])
            ),
        ));
    }
    let (cells, mut notes) = phase_cells(phases);
    if autopilot {
        notes.push("Autopilot adjusts these overnight".to_string());
    }
    if !cells.is_empty() {
        out.push(row("", "levels", with_notes(cells, &notes)));
    }
    out
}

/// The app's names for the API's `bedTimeLevel`, `initialSleepLevel` and
/// `finalSleepLevel`.
fn phase_name(phase: &str) -> &str {
    match phase {
        "bedtime" => "now",
        "initial" => "night",
        "final" => "dawn",
        other => other,
    }
}

/// `weekdays · vibration, warming`, plus `snoozing` while snoozed.
fn alarm_details(a: &Value) -> String {
    let mut parts = vec![if a["repeat"].as_bool() == Some(true) {
        days_summary(&a["days"])
    } else {
        "once".to_string()
    }];
    let mut wake = Vec::new();
    if a["vibration"].as_bool() == Some(true) {
        wake.push("vibration");
    }
    if a["thermal"].as_bool() == Some(true) {
        wake.push("warming");
    }
    if !wake.is_empty() {
        parts.push(wake.join(", "));
    }
    let sep = " · ".dimmed().to_string();
    let mut line = parts.join(&sep);
    if a["snoozing"].as_bool() == Some(true) {
        line.push_str(&format!("{sep}{}", "snoozing".yellow().bold()));
    }
    line
}

/// Status row: `tomorrow 9:15 AM · weekdays · vibration, warming`.
fn alarm_row(a: &Value) -> Option<String> {
    let next = when(a["next"].as_str())?;
    let sep = " · ".dimmed().to_string();
    Some(row(
        "",
        "alarm",
        format!("{}{sep}{}", next.bold(), alarm_details(a)),
    ))
}

/// `8sleep alarm` row: `1  ● on   9:15 AM  weekdays · vibration, warming  next tomorrow 9:15 AM`.
pub fn alarm_line(n: usize, a: &Value) -> String {
    let on = a["enabled"].as_bool() == Some(true);
    let power = if on {
        format!("{} {}", "●".green(), "on ".green().bold())
    } else {
        format!("{} {}", "○".dimmed(), "off".dimmed())
    };
    let time = format!("{:>8}", alarm::clock(a["time"].as_str().unwrap_or("?")));
    let time = if on {
        time.bold().to_string()
    } else {
        time.dimmed().to_string()
    };
    let mut line = format!("  {}  {power}  {time}   {}", n.dimmed(), alarm_details(a));
    if on && let Some(next) = when(a["next"].as_str()) {
        line.push_str(&format!("   {}", format!("next {next}").dimmed()));
    }
    line
}

pub fn days_summary(days: &Value) -> String {
    const DAYS: [(&str, &str); 7] = [
        ("monday", "Mon"),
        ("tuesday", "Tue"),
        ("wednesday", "Wed"),
        ("thursday", "Thu"),
        ("friday", "Fri"),
        ("saturday", "Sat"),
        ("sunday", "Sun"),
    ];
    let on: Vec<bool> = DAYS
        .iter()
        .map(|(k, _)| days[*k].as_bool() == Some(true))
        .collect();
    match on.as_slice() {
        [true, true, true, true, true, true, true] => "daily".into(),
        [true, true, true, true, true, false, false] => "weekdays".into(),
        [false, false, false, false, false, true, true] => "weekends".into(),
        _ => DAYS
            .iter()
            .zip(&on)
            .filter(|(_, on)| **on)
            .map(|((_, short), _)| *short)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

pub fn side_title(side: &str) -> &'static str {
    match side {
        "solo" => "whole bed",
        "left" => "left side",
        "right" => "right side",
        _ => "bed",
    }
}

/// `live · every 5s · updated 5:31:02 PM · ctrl-c to quit`, or the error from
/// the last failed refresh.
pub fn live_footer(every: std::time::Duration, err: Option<&str>) -> String {
    let now = jiff::Zoned::now().strftime("%-I:%M:%S %p").to_string();
    let every = humantime::format_duration(every).to_string();
    match err {
        None => format!("  live · every {every} · updated {now} · ctrl-c to quit")
            .dimmed()
            .to_string(),
        Some(e) => format!(
            "  {} {}",
            format!("refresh failed at {now}:").red().bold(),
            e.red()
        ),
    }
}

pub fn ok(msg: &str) -> String {
    format!("{} {msg}", "✓".green().bold())
}

pub fn error(msg: &str) -> String {
    format!("{} {msg}", "error:".red().bold())
}
