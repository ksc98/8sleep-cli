//! Conversions between the API's raw heating level (-100..=100), the app's
//! -10..=+10 scale and °F.
//!
//! The app maps levels to degrees with a lookup table, not a formula. This
//! table is the one the Home Assistant integration (lukas-clarke/eight_sleep)
//! uses.

#[rustfmt::skip]
const RAW_TO_F: &[(i32, i32)] = &[
    (-100, 55), (-99, 56), (-97, 57), (-95, 58), (-94, 59), (-92, 60), (-90, 61),
    (-86, 62), (-81, 63), (-77, 64), (-72, 65), (-68, 66), (-63, 67), (-58, 68),
    (-54, 69), (-49, 70), (-44, 71), (-40, 72), (-35, 73), (-31, 74), (-26, 75),
    (-21, 76), (-18, 77), (-17, 77), (-12, 78), (-7, 79), (-3, 80), (1, 81),
    (4, 82), (7, 83), (10, 84), (14, 85), (16, 86), (17, 86), (20, 87),
    (23, 88), (26, 89), (29, 90), (32, 91), (35, 92), (38, 93), (41, 94),
    (44, 95), (48, 96), (51, 97), (54, 98), (57, 99), (60, 100), (63, 101),
    (66, 102), (69, 103), (72, 104), (75, 105), (78, 106), (80, 107), (81, 107),
    (85, 108), (88, 109), (92, 110), (100, 111),
];

pub const MIN_F: i32 = 55;
pub const MAX_F: i32 = 111;

/// Raw level to °F, using the nearest table entry.
pub fn raw_to_f(raw: i32) -> i32 {
    RAW_TO_F
        .iter()
        .min_by_key(|(r, _)| (r - raw).abs())
        .map(|&(_, f)| f)
        .unwrap_or(0)
}

/// °F to raw level, using the nearest table entry.
pub fn f_to_raw(f: i32) -> i32 {
    RAW_TO_F
        .iter()
        .min_by_key(|(_, t)| (t - f).abs())
        .map(|&(r, _)| r)
        .unwrap_or(0)
}

/// Raw level on the app's -10..=+10 scale, e.g. -60 -> "-6", 15 -> "+1.5".
pub fn raw_to_app(raw: i32) -> String {
    let v = raw as f64 / 10.0;
    let s = if raw % 10 == 0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    };
    if raw > 0 { format!("+{s}") } else { s }
}

/// Parse a user-supplied temperature into a raw level.
///
/// `68`, `68f` and `68F` are °F (55..=111). `-3`, `+2` and `0` are the app's
/// -10..=+10 scale. The two ranges don't overlap.
pub fn parse(input: &str) -> Result<i32, String> {
    let s = input.trim();
    let (num, forced_f) = match s.strip_suffix(['f', 'F']) {
        Some(n) => (n.trim_end_matches('°'), true),
        None => (s, false),
    };
    let v: f64 = num
        .parse()
        .map_err(|_| format!("not a temperature: {input}"))?;
    if forced_f || v >= MIN_F as f64 {
        let f = v.round() as i32;
        if !(MIN_F..=MAX_F).contains(&f) {
            return Err(format!("{f}°F is outside {MIN_F}..={MAX_F}°F"));
        }
        return Ok(f_to_raw(f));
    }
    if (-10.0..=10.0).contains(&v) {
        return Ok((v * 10.0).round() as i32);
    }
    Err(format!(
        "{input}: use °F ({MIN_F}-{MAX_F}) or the app's -10..+10 scale"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_scales() {
        assert_eq!(parse("68").unwrap(), -58);
        assert_eq!(parse("68F").unwrap(), -58);
        assert_eq!(parse("-3").unwrap(), -30);
        assert_eq!(parse("+2.5").unwrap(), 25);
        assert_eq!(parse("0").unwrap(), 0);
        assert!(parse("30").is_err());
        assert!(parse("120").is_err());
        assert!(parse("40f").is_err());
    }

    #[test]
    fn round_trips_degrees() {
        for f in MIN_F..=MAX_F {
            assert_eq!(raw_to_f(f_to_raw(f)), f);
        }
    }

    #[test]
    fn app_scale_formatting() {
        assert_eq!(raw_to_app(-60), "-6");
        assert_eq!(raw_to_app(15), "+1.5");
        assert_eq!(raw_to_app(0), "0");
    }
}
