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
/// Chevrons pointing from a label to its marker: Nerd Font `md-chevron_down`
/// from a label above the bar, `md-chevron_up` from one below it. They need
/// a Nerd Font; Ghostty on both of the user's machines uses JetBrainsMono NF.
const CARET_ABOVE: &str = "\u{f0140}";
const CARET_BELOW: &str = "\u{f0143}";

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

/// Columns before a `row` value: two spaces, the marker, a space, the label.
const ROW_INDENT: usize = 4 + LABEL_WIDTH;
/// `cool ` before the first gauge cell.
const GAUGE_LEAD: usize = 5;

/// `<chevron> target 81°F 0` with the chevron at `col`, directly over (or under) its
/// marker on the bar. Near the warm end it flips to `target 81°F 0 <chevron>` so it
/// doesn't run past `end`.
fn bar_label(
    name: &str,
    raw: i32,
    bold: bool,
    precise: bool,
    caret: &str,
    col: usize,
    end: usize,
) -> String {
    let f = format!("{}°F", temp::raw_to_f(raw));
    let app = if precise {
        temp::raw_to_app_precise(raw)
    } else {
        temp::raw_to_app(raw)
    };
    let width = [name, &f, &app]
        .iter()
        .map(|p| p.chars().count())
        .sum::<usize>()
        + 4;
    let fc = if bold {
        colored_level(&f, raw).bold().to_string()
    } else {
        colored_level(&f, raw)
    };
    let text = format!("{} {fc} {}", name.dimmed(), app.dimmed());
    let caret = caret.dimmed();
    if col + width <= end {
        format!("{}{caret} {text}", " ".repeat(col))
    } else {
        format!(
            "{}{text} {caret}",
            " ".repeat((col + 1).saturating_sub(width))
        )
    }
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

/// `● active` (green) or `○ inactive` (dimmed), plus the state when the bed
/// runs something other than its smart schedule.
pub fn power(s: &Value) -> String {
    let on = s["on"].as_bool() == Some(true);
    let mut out = if on {
        format!("{} {}", "●".green(), "active".green().bold())
    } else {
        format!("{} {}", "○".dimmed(), "inactive".dimmed())
    };
    // Smart phases show up in the schedule row; anything else is unusual.
    let state = s["state"].as_str().unwrap_or("?");
    if on && !state.starts_with("smart") {
        out.push_str(&format!(" {}", state.dimmed()));
    }
    out
}

/// Header line: model, connectivity, water and, for a solo bed, `power`.
pub fn device_line(dev: &Value, water: &str, power: Option<&str>) -> String {
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
    let mut line = format!("{}{sep}{online}{sep}{water}", model.bold());
    if let Some(p) = power {
        line.push_str(&format!("{sep}{p}"));
    }
    line
}

/// The block for one side of the bed. `title` heads it (`left side (you)
/// ● active`) on a split bed; a solo bed passes `None` and shows its power
/// elsewhere.
pub fn side_block(title: Option<&str>, you: bool, s: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(title) = title {
        let mut head = format!("  {}", title.bold());
        if you {
            head.push_str(&format!(" {}", "(you)".dimmed()));
        }
        if s["state"] == "unassigned" {
            head.push_str(&format!("  {}", "unassigned".dimmed()));
            return vec![head];
        }
        head.push_str(&format!("   {}", power(s)));
        lines.push(head);
    }
    let on = s["on"].as_bool() == Some(true);
    let target = s["target"].as_i64().map(|v| v as i32);
    // The API keeps reporting the current level while the bed is off.
    let now = s["level"].as_i64().map(|v| v as i32);
    if let Some(t) = target {
        // Target labelled above the bar, current below, each starting at
        // (or ending on) its marker's column.
        let cell0 = ROW_INDENT + GAUGE_LEAD;
        let end = cell0 + GAUGE_CELLS as usize + GAUGE_LEAD;
        let col = |raw| cell0 + gauge_cell(raw) as usize;
        lines.push(bar_label(
            "target",
            t,
            true,
            false,
            CARET_ABOVE,
            col(t),
            end,
        ));
        lines.push(row("", "", gauge(t, now, on)));
        if let Some(n) = now {
            lines.push(bar_label(
                "current",
                n,
                false,
                true,
                CARET_BELOW,
                col(n),
                end,
            ));
        }
    }
    lines.extend(phase_rows(s));
    if let Some(bedtime) = bedtime_row(s) {
        lines.push(bedtime);
    }
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
    let cells = phase_cells(&s["phases"]);
    if cells.is_empty() {
        return Vec::new();
    }
    vec![row("", "schedule", cells)]
}

/// How long before bedtime the app turns the pod on ("Pod turns on: 1h
/// before bedtime"). The API only stores the bedtime.
const POD_ON_LEAD_HOURS: i64 = 1;

/// `bedtime  1:45 AM · daily · cools at 12:45 AM · 6h 40m`. The verb
/// compares the schedule's `now` level with the bed's current level: that's
/// where the bed heads when the pod turns on.
fn bedtime_row(s: &Value) -> Option<String> {
    let time = alarm::clock(s["bedtime"]["time"].as_str()?);
    let start = s["phases"][0]["scheduled"].as_i64();
    let current = s["level"].as_i64();
    let verb = match (start, current) {
        (Some(a), Some(b)) if a < b => "cools",
        (Some(a), Some(b)) if a > b => "warms",
        _ => "starts",
    };
    let sep = " · ".dimmed().to_string();
    let mut line = format!(
        "{}{sep}{}",
        time.bold(),
        days_summary(&s["bedtime"]["days"])
    );
    let next: Option<jiff::Timestamp> = s["next_bedtime"].as_str().and_then(|t| t.parse().ok());
    if let Some(next) = next {
        let on_at = next - jiff::SignedDuration::from_hours(POD_ON_LEAD_HOURS);
        let now = jiff::Timestamp::now();
        let clock = on_at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%-I:%M %p")
            .to_string();
        let tail = if on_at <= now {
            format!("{verb} now, since {clock}")
        } else {
            format!("{verb} at {clock}{sep}{}", eta((on_at - now).get_seconds()))
        };
        line.push_str(&format!("{sep}{}", tail.dimmed()));
    }
    Some(row("", "bedtime", line))
}

/// `8h 30m`, `45m`, `2d 3h`, `<1m`.
fn eta(secs: i64) -> String {
    let m = secs.max(0) / 60;
    let (d, h, m) = (m / 1440, (m / 60) % 24, m % 60);
    match (d, h, m) {
        (0, 0, 0) => "<1m".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// `now 55°F -10 (0) │ night 78°F -1 │ dawn 82°F 0`: each phase's
/// level, with the saved one in parentheses when it was set by hand.
fn phase_cells(phases: &Value) -> String {
    let mut cells = Vec::new();
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
        let mut cell = format!("{label} {f} {}", temp::raw_to_app(level).dimmed());
        // A level set by hand while the phase runs: show the saved one too.
        if p["overridden"].as_bool() == Some(true)
            && let Some(sched) = p["scheduled"].as_i64()
        {
            let saved = format!("({})", temp::raw_to_app(sched as i32));
            cell.push_str(&format!(" {}", saved.dimmed().italic()));
        }
        cells.push(cell);
    }
    cells.join(&" │ ".dimmed().to_string())
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
    let cells = phase_cells(phases);
    if !cells.is_empty() {
        let mut line = cells;
        if autopilot {
            line.push_str(&format!(
                "   {}",
                "Autopilot adjusts these overnight".dimmed()
            ));
        }
        out.push(row("", "levels", line));
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

/// `weekdays · vibration 50% intense, warming 50`, plus `snoozing` while snoozed.
fn alarm_details(a: &Value) -> String {
    let mut parts = vec![if a["repeat"].as_bool() == Some(true) {
        days_summary(&a["days"])
    } else {
        "once".to_string()
    }];
    let mut wake = Vec::new();
    if a["vibration"].as_bool() == Some(true) {
        let mut v = "vibration".to_string();
        if let Some(p) = a["vibration_power"].as_i64() {
            v.push_str(&format!(" {p}%"));
        }
        if let Some(pat) = a["vibration_pattern"].as_str() {
            v.push_str(&format!(" {}", pat.to_ascii_lowercase()));
        }
        wake.push(v);
    }
    if a["thermal"].as_bool() == Some(true) {
        // `thermal.level`'s scale isn't known, so it's shown as the raw number.
        wake.push(match a["thermal_level"].as_i64() {
            Some(l) => format!("warming {l}"),
            None => "warming".to_string(),
        });
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

/// Status row: `8:45 AM · weekdays · vibration 50% intense, warming 50`.
fn alarm_row(a: &Value) -> Option<String> {
    a["next"].as_str()?;
    let next = alarm::clock(a["time"].as_str()?);
    let sep = " · ".dimmed().to_string();
    Some(row(
        "",
        "alarm",
        format!("{}{sep}{}", next.bold(), alarm_details(a)),
    ))
}

/// `8sleep alarm` row: `1  ● on   8:45 AM   weekdays · vibration 50% intense, warming 50   next tomorrow 8:45 AM`.
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

#[cfg(test)]
mod tests {
    use super::{CARET_ABOVE, CARET_BELOW, bar_label, eta};

    /// Column of the caret once color codes are stripped.
    fn caret_col(line: &str) -> usize {
        let plain = String::from_utf8(strip_ansi(line.as_bytes())).unwrap();
        plain
            .chars()
            .position(|c| [CARET_ABOVE, CARET_BELOW].contains(&c.to_string().as_str()))
            .unwrap()
    }

    fn strip_ansi(b: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == 0x1b {
                while i < b.len() && b[i] != b'm' {
                    i += 1;
                }
            } else {
                out.push(b[i]);
            }
            i += 1;
        }
        out
    }

    #[test]
    fn label_caret_points_at_its_marker() {
        // Room to the right: caret first, at the marker's column.
        assert_eq!(
            caret_col(&bar_label("target", 0, true, false, CARET_ABOVE, 30, 70)),
            30
        );
        // Near the warm end the label flips left; the caret stays on the marker.
        let flipped = bar_label("current", 100, false, true, CARET_BELOW, 65, 70);
        assert_eq!(caret_col(&flipped), 65);
        assert!(strip_ansi(flipped.as_bytes()).ends_with(CARET_BELOW.as_bytes()));
    }

    #[test]
    fn formats_eta() {
        assert_eq!(eta(30), "<1m");
        assert_eq!(eta(45 * 60), "45m");
        assert_eq!(eta(8 * 3600 + 30 * 60 + 59), "8h 30m");
        assert_eq!(eta(2 * 86400 + 3 * 3600 + 120), "2d 3h");
        assert_eq!(eta(-5), "<1m");
    }
}
