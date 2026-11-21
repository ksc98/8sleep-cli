//! The night as a picture: a stage bar from getting into bed to getting out,
//! on a clock axis snapped to whole hours, with a snoring row above it and
//! heart rate and HRV sparklines on the same axis.
//!
//! This file is identical in 8sleep-cli and oura-cli (`src/timeline.rs`), so
//! `8sleep sleep` and `oura sleep` draw the same colors on the same grid and
//! their bars line up column for column. It works on unix seconds; each CLI
//! turns its API's night into a [`Night`] and passes its UTC offset.

use std::fmt::Write;

/// A sleep stage, in hypnogram order (top to bottom), plus time out of bed
/// between two sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Awake,
    Rem,
    Light,
    Deep,
    Out,
}

impl Stage {
    const ALL: [Stage; 5] = [
        Stage::Awake,
        Stage::Rem,
        Stage::Light,
        Stage::Deep,
        Stage::Out,
    ];

    /// The name `--json` uses.
    pub fn name(self) -> &'static str {
        match self {
            Stage::Awake => "awake",
            Stage::Rem => "rem",
            Stage::Light => "light",
            Stage::Deep => "deep",
            Stage::Out => "out",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|st| st.name() == s)
    }

    fn label(self) -> &'static str {
        match self {
            Stage::Rem => "REM",
            Stage::Out => "out of bed",
            s => s.name(),
        }
    }

    /// Catppuccin Macchiato: peach, mauve, blue, a deeper blue, surface2.
    fn rgb(self) -> Rgb {
        match self {
            Stage::Awake => (245, 169, 127),
            Stage::Rem => (198, 160, 246),
            Stage::Light => (138, 173, 244),
            Stage::Deep => (84, 104, 224),
            Stage::Out => (91, 96, 120),
        }
    }

    /// Without color the bar becomes a hypnogram: awake on top, deep at the
    /// bottom.
    fn glyph(self) -> char {
        match self {
            Stage::Awake => '█',
            Stage::Rem => '▆',
            Stage::Light => '▄',
            Stage::Deep => '▂',
            Stage::Out => '·',
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

type Rgb = (u8, u8, u8);
const SNORE: Rgb = (238, 212, 159);
/// Snoring and heavy snoring marks: low and high bumps on the bar in
/// color, letters without it (the blocks there already mean stages).
const SNORE_MARKS: [[&str; 2]; 2] = [["▂", "▅"], ["z", "Z"]];
const HEAVY_SNORE: Rgb = (237, 135, 150);
const HEART: Rgb = (237, 135, 150);
const LOWEST: Rgb = (238, 212, 159);
const HRV: Rgb = (166, 218, 149);

/// `[start, end)` in unix seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: i64,
    pub end: i64,
    pub stage: Stage,
}

/// Snoring from `start` to `end`; `heavy` for the louder kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snore {
    pub start: i64,
    pub end: i64,
    pub heavy: bool,
}

/// Samples taken every `interval` seconds, each `(start, value)`.
#[derive(Clone, Debug, Default)]
pub struct Series {
    pub interval: i64,
    pub points: Vec<(i64, f64)>,
}

#[derive(Clone, Debug, Default)]
pub struct Night {
    pub in_bed: (i64, i64),
    pub stages: Vec<Span>,
    pub snoring: Vec<Snore>,
    pub heart: Series,
    pub hrv: Series,
    /// When the lowest heart rate was measured.
    pub lowest: Option<i64>,
}

pub struct Style {
    /// Terminal columns.
    pub width: usize,
    pub color: bool,
    /// Print the color legend under the sparklines.
    pub legend: bool,
}

/// Columns before the bar: the report rows' two-space indent, 10-wide label
/// and a space.
pub const GUTTER: usize = 13;
/// Room after the bar for the last hour label.
const TAIL: usize = 4;
/// At most one cell per 5 minutes, Oura's hypnogram and both trackers'
/// heart rate resolution.
const MAX_PER_HOUR: usize = 12;

/// The time axis: whole local hours around the night, cut into cells.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    start: i64,
    hours: usize,
    per_hour: usize,
}

impl Axis {
    fn new(in_bed: (i64, i64), offset: &dyn Fn(i64) -> i64, width: usize) -> Axis {
        let floor = |t: i64| {
            let o = offset(t);
            (t + o).div_euclid(3600) * 3600 - o
        };
        let start = floor(in_bed.0);
        let mut end = floor(in_bed.1);
        if end < in_bed.1 {
            end += 3600;
        }
        let hours = ((end - start) / 3600).max(1) as usize;
        let room = width.saturating_sub(GUTTER + TAIL);
        let per_hour = (room / hours).clamp(1, MAX_PER_HOUR);
        Axis {
            start,
            hours,
            per_hour,
        }
    }

    fn cells(&self) -> usize {
        self.hours * self.per_hour
    }

    fn cell_secs(&self) -> f64 {
        3600.0 / self.per_hour as f64
    }

    /// The cell holding time `t`, if it's on the axis.
    fn cell(&self, t: f64) -> Option<usize> {
        let i = ((t - self.start as f64) / self.cell_secs()).floor();
        (i >= 0.0 && (i as usize) < self.cells()).then_some(i as usize)
    }

    /// Each cell's overlap with `[start, end)`, in seconds.
    fn overlaps(&self, start: i64, end: i64) -> impl Iterator<Item = (usize, f64)> + '_ {
        let len = self.cell_secs();
        let (s, e) = (
            (start - self.start) as f64 / len,
            (end - self.start) as f64 / len,
        );
        let from = s.floor().max(0.0) as usize;
        let to = (e.ceil().max(0.0) as usize).min(self.cells());
        (from..to).filter_map(move |i| {
            let lo = s.max(i as f64);
            let hi = e.min(i as f64 + 1.0);
            (hi > lo).then_some((i, (hi - lo) * len))
        })
    }
}

/// The night's picture, as lines to print after the `stages` row: the
/// snoring row (when there was any), the stage bar, the hour axis, the
/// sparklines and, with `style.legend`, the legend.
pub fn render(n: &Night, offset: &dyn Fn(i64) -> i64, style: &Style) -> Vec<String> {
    if n.in_bed.1 <= n.in_bed.0 || n.stages.is_empty() {
        return Vec::new();
    }
    let axis = Axis::new(n.in_bed, offset, style.width);
    let pad = " ".repeat(GUTTER);
    let mut out = Vec::new();

    let snore = snore_cells(&axis, &n.snoring);
    let [light_mark, heavy_mark] = SNORE_MARKS[usize::from(!style.color)];
    if snore.iter().any(|&s| s > 0) {
        let cells = snore.iter().map(|&s| match s {
            0 => " ".into(),
            1 => paint(light_mark, Some(SNORE), None, style.color),
            _ => paint(heavy_mark, Some(HEAVY_SNORE), None, style.color),
        });
        out.push(format!("{pad}{}", join_cells(cells)));
    }

    let cover = coverage(&axis, &n.stages);
    let len = axis.cell_secs();
    let bar = cover.iter().map(|c| {
        if style.color {
            color_cell(c, len)
        } else {
            plain_cell(c, len).to_string()
        }
    });
    out.push(format!("{pad}{}", join_cells(bar)));
    out.push(format!(
        "{pad}{}",
        dim(&hour_labels(&axis, offset), style.color)
    ));

    let lowest = n
        .lowest
        .and_then(|t| axis.cell(t as f64 + n.heart.interval as f64 / 2.0));
    for (label, series, rgb) in [("HR", &n.heart, HEART), ("HRV", &n.hrv, HRV)] {
        let mark = if label == "HR" { lowest } else { None };
        if let Some(line) = sparkline(&axis, label, series, rgb, mark, style.color) {
            out.push(line);
        }
    }

    if style.legend {
        let mut items: Vec<String> = [Stage::Awake, Stage::Rem, Stage::Light, Stage::Deep]
            .into_iter()
            .map(|s| legend_item(s, style.color))
            .collect();
        if cover.iter().any(|c| c[Stage::Out.index()] > 0.0) {
            items.push(legend_item(Stage::Out, style.color));
        }
        if snore.iter().any(|&s| s > 0) {
            items.push(format!(
                "{} {}",
                paint(light_mark, Some(SNORE), None, style.color),
                dim("snoring", style.color)
            ));
            items.push(format!(
                "{} {}",
                paint(heavy_mark, Some(HEAVY_SNORE), None, style.color),
                dim("heavy", style.color)
            ));
        }
        out.push(format!("{pad}{}", items.join("  ")));
    }
    out
}

/// Seconds of each stage (by `Stage::index`) in a cell, and how many
/// seconds into the cell each first appears.
#[derive(Clone, Copy, Debug, Default)]
struct Cover {
    secs: [f64; 5],
    first: [f64; 5],
}

impl std::ops::Index<usize> for Cover {
    type Output = f64;
    fn index(&self, i: usize) -> &f64 {
        &self.secs[i]
    }
}

fn coverage(axis: &Axis, stages: &[Span]) -> Vec<Cover> {
    let mut cells = vec![
        Cover {
            first: [f64::INFINITY; 5],
            ..Default::default()
        };
        axis.cells()
    ];
    for s in stages {
        let i = s.stage.index();
        for (c, secs) in axis.overlaps(s.start, s.end) {
            let cell = &mut cells[c];
            cell.secs[i] += secs;
            let into = s.start as f64 - (axis.start as f64 + c as f64 * axis.cell_secs());
            cell.first[i] = cell.first[i].min(into.max(0.0));
        }
    }
    cells
}

/// The two biggest parts of a cell, left to right, where `None` is time
/// outside every span; `None` for an empty cell.
#[allow(clippy::type_complexity)]
fn split(c: &Cover, len: f64) -> Option<((Option<Stage>, f64), Option<(Option<Stage>, f64)>)> {
    let covered: f64 = c.secs.iter().sum();
    let gap = (len - covered).max(0.0);
    let mut parts: Vec<(Option<Stage>, f64, f64)> = Stage::ALL
        .into_iter()
        .filter(|s| c[s.index()] > 0.0)
        .map(|s| (Some(s), c[s.index()], c.first[s.index()]))
        .collect();
    if gap > 0.0 {
        // Uncovered time sits at the cell's start when the night begins
        // inside it, else at its end.
        let earliest = c.first.iter().copied().fold(f64::INFINITY, f64::min);
        parts.push((None, gap, if earliest > 0.0 { -1.0 } else { f64::INFINITY }));
    }
    parts.sort_by(|a, b| b.1.total_cmp(&a.1));
    parts.truncate(2);
    // A sliver under 1/16 of the cell rounds away.
    parts.retain(|p| p.1 >= len / 16.0);
    match parts.as_slice() {
        [] => None,
        [(None, ..)] => None,
        [(s, secs, _)] => Some(((*s, *secs), None)),
        [a, b] => {
            let (l, r) = if a.2 <= b.2 { (a, b) } else { (b, a) };
            Some(((l.0, l.1), Some((r.0, r.1))))
        }
        _ => unreachable!(),
    }
}

/// A cell in color: solid, or two stages split at eighth-cell resolution
/// (the left one as a left block in its color over the right one's
/// background).
fn color_cell(c: &Cover, len: f64) -> String {
    const LEFT: [&str; 7] = ["▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    match split(c, len) {
        None => " ".into(),
        Some(((Some(s), _), None)) => paint("█", Some(s.rgb()), None, true),
        Some(((None, _), None)) => " ".into(),
        Some(((l, ls), Some((r, rs)))) => {
            let eighths = ((ls / (ls + rs)) * 8.0).round().clamp(1.0, 7.0) as usize;
            let block = LEFT[eighths - 1];
            match (l, r) {
                (Some(l), Some(r)) => paint(block, Some(l.rgb()), Some(r.rgb()), true),
                (Some(l), None) => paint(block, Some(l.rgb()), None, true),
                // Blank on the left: draw the blank part as the glyph in
                // reverse video, so the stage fills the rest of the cell.
                (None, Some(r)) => format!("\x1b[7m{}", paint(block, Some(r.rgb()), None, true)),
                (None, None) => " ".into(),
            }
        }
    }
}

/// A cell without color: the glyph of the stage that fills most of it.
fn plain_cell(c: &Cover, len: f64) -> char {
    let covered: f64 = c.secs.iter().sum();
    let best = Stage::ALL
        .into_iter()
        .max_by(|a, b| c[a.index()].total_cmp(&c[b.index()]))
        .filter(|s| c[s.index()] > 0.0);
    match best {
        Some(s) if covered >= len / 2.0 => s.glyph(),
        _ => ' ',
    }
}

/// The loudest snoring in each cell: 0 none, 1 snoring, 2 heavy. Short
/// spans still mark their cell.
fn snore_cells(axis: &Axis, snoring: &[Snore]) -> Vec<u8> {
    let mut cells = vec![0u8; axis.cells()];
    for s in snoring {
        for (c, _) in axis.overlaps(s.start, s.end) {
            cells[c] = cells[c].max(if s.heavy { 2 } else { 1 });
        }
    }
    cells
}

/// `HR 43–62  ▆▅▄▃▂▁…`: each cell is the mean of the samples centered in it,
/// scaled from the lowest cell to the highest; `mark` highlights a cell.
fn sparkline(
    axis: &Axis,
    label: &str,
    s: &Series,
    rgb: Rgb,
    mark: Option<usize>,
    color: bool,
) -> Option<String> {
    const LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let mut sums = vec![(0.0, 0u32); axis.cells()];
    for &(t, v) in &s.points {
        if let Some(c) = axis.cell(t as f64 + s.interval as f64 / 2.0) {
            sums[c].0 += v;
            sums[c].1 += 1;
        }
    }
    let cells: Vec<Option<f64>> = sums
        .iter()
        .map(|&(sum, n)| (n > 0).then(|| sum / n as f64))
        .collect();
    let values = cells.iter().flatten();
    let lo = values.clone().copied().reduce(f64::min)?;
    let hi = values.copied().reduce(f64::max)?;
    let line = cells.iter().enumerate().map(|(i, v)| match v {
        None => " ".into(),
        Some(v) => {
            let level = if hi > lo {
                ((v - lo) / (hi - lo) * 7.0).round() as usize
            } else {
                3
            };
            if mark == Some(i) && color {
                format!("\x1b[1m{}", paint(LEVELS[level], Some(LOWEST), None, true))
            } else {
                paint(LEVELS[level], Some(rgb), None, color)
            }
        }
    });
    // The scale's bounds, widened to whole numbers.
    let label = format!("{label} {}–{}", lo.floor(), hi.ceil());
    Some(format!(
        "  {}{} {}",
        dim(&label, color),
        " ".repeat(10usize.saturating_sub(label.chars().count())),
        join_cells(line)
    ))
}

/// `2am         3am …`, each label starting at its hour's first cell.
fn hour_labels(axis: &Axis, offset: &dyn Fn(i64) -> i64) -> String {
    // Labels are up to 4 wide and need a space between them.
    let every = 5usize.div_ceil(axis.per_hour);
    let mut row = vec![' '; axis.cells() + TAIL];
    let mut free = 0;
    for h in (0..=axis.hours).step_by(every) {
        let t = axis.start + h as i64 * 3600;
        let hour = (t + offset(t)).div_euclid(3600).rem_euclid(24);
        let label = match hour {
            0 => "12am".to_string(),
            1..=11 => format!("{hour}am"),
            12 => "12pm".to_string(),
            _ => format!("{}pm", hour - 12),
        };
        let at = h * axis.per_hour;
        if at < free || at + label.len() > row.len() {
            continue;
        }
        for (i, ch) in label.chars().enumerate() {
            row[at + i] = ch;
        }
        free = at + label.len() + 1;
    }
    row.into_iter().collect::<String>().trim_end().to_string()
}

fn legend_item(s: Stage, color: bool) -> String {
    let swatch = if color {
        paint("█", Some(s.rgb()), None, true)
    } else {
        s.glyph().to_string()
    };
    format!("{swatch} {}", dim(s.label(), color))
}

/// Cells joined, with trailing blanks dropped.
fn join_cells(cells: impl Iterator<Item = String>) -> String {
    let mut v: Vec<String> = cells.collect();
    while v.last().is_some_and(|c| c == " ") {
        v.pop();
    }
    v.concat()
}

fn paint(text: &str, fg: Option<Rgb>, bg: Option<Rgb>, color: bool) -> String {
    if !color {
        return text.to_string();
    }
    let mut s = String::new();
    if let Some((r, g, b)) = fg {
        let _ = write!(s, "\x1b[38;2;{r};{g};{b}m");
    }
    if let Some((r, g, b)) = bg {
        let _ = write!(s, "\x1b[48;2;{r};{g};{b}m");
    }
    format!("{s}{text}\x1b[0m")
}

fn dim(text: &str, color: bool) -> String {
    if color && !text.is_empty() {
        format!("\x1b[2m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;
    /// 2026-11-20 00:00 at UTC-8, as unix seconds.
    const MIDNIGHT: i64 = 1_763_625_600;

    fn pst(_: i64) -> i64 {
        -8 * H
    }

    fn at(h: f64) -> i64 {
        MIDNIGHT + (h * H as f64) as i64
    }

    fn plain(width: usize) -> Style {
        Style {
            width,
            color: false,
            legend: true,
        }
    }

    fn night() -> Night {
        // 2:52 in bed, light until 4:00, deep until 5:00, REM until 6:00,
        // awake until 6:30, out until 7:00, light until 8:49.
        let span = |a: f64, b: f64, stage| Span {
            start: at(a),
            end: at(b),
            stage,
        };
        Night {
            in_bed: (at(2.0 + 52.0 / 60.0), at(8.0 + 49.0 / 60.0)),
            stages: vec![
                span(2.0 + 52.0 / 60.0, 4.0, Stage::Light),
                span(4.0, 5.0, Stage::Deep),
                span(5.0, 6.0, Stage::Rem),
                span(6.0, 6.5, Stage::Awake),
                span(6.5, 7.0, Stage::Out),
                span(7.0, 8.0 + 49.0 / 60.0, Stage::Light),
            ],
            snoring: vec![
                Snore {
                    start: at(4.0),
                    end: at(4.0) + 60,
                    heavy: false,
                },
                Snore {
                    start: at(4.5),
                    end: at(4.5) + 240,
                    heavy: true,
                },
            ],
            heart: Series {
                interval: 300,
                points: (0..60)
                    .map(|i| (at(3.0) + i * 300, 60.0 - (i % 30) as f64))
                    .collect(),
            },
            hrv: Series::default(),
            lowest: Some(at(3.0) + 29 * 300),
        }
    }

    #[test]
    fn snaps_the_axis_to_local_hours() {
        let n = night();
        let a = Axis::new(n.in_bed, &pst, 200);
        assert_eq!(a.start, at(2.0));
        assert_eq!(a.hours, 7);
        assert_eq!(a.per_hour, 12);
        // 80 columns leave 63 cells: 9 per hour.
        assert_eq!(Axis::new(n.in_bed, &pst, 80).per_hour, 9);
        // Ending on the hour doesn't add an empty hour.
        assert_eq!(Axis::new((at(2.5), at(9.0)), &pst, 200).hours, 7);
    }

    #[test]
    fn draws_a_plain_hypnogram() {
        let lines = render(&night(), &pst, &plain(200));
        let (snore, bar, axis) = (&lines[0], &lines[1], &lines[2]);
        let cells: Vec<char> = bar.chars().skip(GUTTER).collect();
        // 2:52 falls in the 2:50 cell, which is mostly light.
        assert_eq!(cells[..10].iter().collect::<String>().trim(), "");
        assert_eq!(cells[10], '▄');
        assert_eq!(cells[24], '▂');
        assert_eq!(cells[36], '▆');
        assert_eq!(cells[48], '█');
        assert_eq!(cells[54], '·');
        assert_eq!(cells[60], '▄');
        // 8:49 ends the 8:45 cell 4/5 full.
        assert_eq!(cells.len(), 6 * 12 + 10);
        assert_eq!(&axis[GUTTER..GUTTER + 3], "2am");
        assert_eq!(&axis[GUTTER + 12..GUTTER + 15], "3am");
        assert!(axis.ends_with("9am"), "{axis}");
        // Snoring at 4:00 (one minute) and heavy at 4:30.
        let marks: Vec<char> = snore.chars().skip(GUTTER).collect();
        assert_eq!(marks[24], 'z');
        assert_eq!(marks[30], 'Z');
        assert!(lines.last().unwrap().contains("out of bed"), "{lines:?}");
    }

    #[test]
    fn splits_cells_at_eighths_in_color() {
        let n = Night {
            in_bed: (at(2.0), at(3.0)),
            stages: vec![
                Span {
                    start: at(2.0),
                    end: at(2.0) + 120,
                    stage: Stage::Awake,
                },
                Span {
                    start: at(2.0) + 120,
                    end: at(3.0),
                    stage: Stage::Deep,
                },
            ],
            ..Default::default()
        };
        let style = Style {
            width: 200,
            color: true,
            legend: false,
        };
        let bar = &render(&n, &pst, &style)[0];
        // 2 of 5 minutes awake: 3/8 block, peach on deep blue.
        assert!(
            bar.contains("\x1b[38;2;245;169;127m\x1b[48;2;84;104;224m▍"),
            "{bar:?}"
        );
        // A night starting mid-cell fills the cell's right part.
        let late = Night {
            in_bed: (at(2.0) + 120, at(3.0)),
            stages: vec![Span {
                start: at(2.0) + 120,
                end: at(3.0),
                stage: Stage::Light,
            }],
            ..Default::default()
        };
        let bar = &render(&late, &pst, &style)[0];
        assert!(bar.contains("\x1b[7m\x1b[38;2;138;173;244m▍"), "{bar:?}");
    }

    #[test]
    fn sparkline_scales_and_marks_the_lowest() {
        let lines = render(&night(), &pst, &plain(200));
        let hr = lines.iter().find(|l| l.starts_with("  HR ")).unwrap();
        assert!(hr.starts_with("  HR 31–60   "), "{hr}");
        let cells: Vec<char> = hr.chars().skip(GUTTER).collect();
        assert_eq!(cells[12], '█');
        assert_eq!(cells[12 + 29], '▁');
        let style = Style {
            width: 200,
            color: true,
            legend: false,
        };
        let hr = render(&night(), &pst, &style)
            .into_iter()
            .find(|l| l.contains("HR "))
            .unwrap();
        assert!(hr.contains("\x1b[1m\x1b[38;2;238;212;159m▁"), "{hr:?}");
    }

    #[test]
    fn labels_fewer_hours_when_narrow() {
        let n = night();
        let a = Axis::new(n.in_bed, &pst, 40);
        assert_eq!(a.per_hour, 3);
        let row = hour_labels(&a, &pst);
        assert!(row.starts_with("2am   4am"), "{row}");
    }
}
