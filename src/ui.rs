//! Terminal rendering. Everything is printed through `anstream`, which strips
//! the ANSI codes when stdout isn't a terminal or `NO_COLOR` is set.

use clap::builder::styling::{AnsiColor, Effects, Styles};
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::temp;

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

const GAUGE_CELLS: i32 = 21; // one cell per app-scale step, -10..=+10

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
fn temp_pair(raw: i32, bold: bool) -> String {
    let f = format!("{}°F", temp::raw_to_f(raw));
    let f = if bold {
        colored_level(&f, raw).bold().to_string()
    } else {
        colored_level(&f, raw)
    };
    format!("{f}  {}", format!("{:>4}", temp::raw_to_app(raw)).dimmed())
}

/// A cool→warm bar with a marker at `raw`.
fn gauge(raw: i32) -> String {
    let pos = ((raw.clamp(-100, 100) + 100) as f64 / 10.0).round() as i32;
    let bar: String = (0..GAUGE_CELLS)
        .map(|i| {
            let (r, g, b) = level_rgb(i * 10 - 100);
            if i == pos {
                "●".bright_white().bold().to_string()
            } else {
                "━".truecolor(r, g, b).to_string()
            }
        })
        .collect();
    format!("{} {bar} {}", "cool".dimmed(), "warm".dimmed())
}

fn row(label: &str, value: impl std::fmt::Display) -> String {
    format!("  {}{value}", format!("{label:<LABEL_WIDTH$}").dimmed())
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
    let mut line = format!("{}{sep}{online}{sep}{water}", model.bold());
    if let Some(p) = when(dev["lastPrime"].as_str()) {
        line.push_str(&format!("{sep}{}", format!("primed {p}").dimmed()));
    }
    line
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
    let state = s["state"].as_str().unwrap_or("?").replace(':', " · ");
    head.push_str(&format!("   {power}   {}", state.dimmed()));

    let mut lines = vec![head];
    if let Some(target) = s["target"].as_i64().map(|v| v as i32) {
        lines.push(row(
            "target",
            format!("{}   {}", temp_pair(target, true), gauge(target)),
        ));
    }
    if on && let Some(level) = s["level"].as_i64().map(|v| v as i32) {
        lines.push(row("now", temp_pair(level, false)));
    }
    if s["away"].as_bool() == Some(true) {
        lines.push(row("away", "on".yellow().bold()));
    }
    if let Some(next) = when(s["next_bedtime"].as_str()) {
        lines.push(row("bedtime", next));
    }
    lines
}

pub fn side_title(side: &str) -> &'static str {
    match side {
        "solo" => "whole bed",
        "left" => "left side",
        "right" => "right side",
        _ => "bed",
    }
}

pub fn ok(msg: &str) -> String {
    format!("{} {msg}", "✓".green().bold())
}

pub fn error(msg: &str) -> String {
    format!("{} {msg}", "error:".red().bold())
}
