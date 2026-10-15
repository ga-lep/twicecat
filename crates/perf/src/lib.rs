//! Performance indicators for tests (docs/testing.md, tier 4).
//!
//! A test that measures a speed (frames per second, a median time per batch)
//! calls [`record`] with the value and what is expected of it. The call prints
//! one `perf:` line, appends one JSON object to `$TWICECAT_PERF_FILE` when that
//! is set (the perf workflow reads it), and under GitHub Actions marks a value
//! outside its expectation with a `::warning` annotation. It never fails the
//! test and never panics: speed depends on the machine, so it is an indicator
//! and not a gate. A behaviour that must hold (order, content, a stream that
//! stalls) is asserted by the test itself, not recorded here.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// What a value is expected to be. Outside it, a run is marked, not failed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Expect {
    /// A value of at least this: a rate.
    AtLeast(f64),
    /// A value of at most this: a time or a count of late events.
    AtMost(f64),
    /// No expectation yet: the value is only reported.
    Report,
}

impl Expect {
    /// Whether `value` meets the expectation. A value that is not a number
    /// never does, except under [`Expect::Report`].
    pub fn holds(self, value: f64) -> bool {
        match self {
            Expect::AtLeast(least) => value >= least,
            Expect::AtMost(most) => value <= most,
            Expect::Report => true,
        }
    }

    fn describe(self) -> String {
        match self {
            Expect::AtLeast(least) => format!("expected at least {least}"),
            Expect::AtMost(most) => format!("expected at most {most}"),
            Expect::Report => "no expectation".into(),
        }
    }

    fn json(self) -> String {
        match self {
            Expect::AtLeast(least) => format!("{{\"op\":\"min\",\"value\":{}}}", number(least)),
            Expect::AtMost(most) => format!("{{\"op\":\"max\",\"value\":{}}}", number(most)),
            Expect::Report => "null".into(),
        }
    }
}

/// Records an indicator. Returns whether it is within its expectation, for a
/// test that wants to print more; nothing depends on it.
pub fn record(name: &str, value: f64, unit: &str, expect: Expect) -> bool {
    let file = std::env::var_os("TWICECAT_PERF_FILE");
    let actions = std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true");
    record_with(
        file.as_deref().map(Path::new),
        actions,
        name,
        value,
        unit,
        expect,
    )
}

/// A time as milliseconds, expected to be at most `at_most`.
pub fn record_duration(name: &str, value: Duration, at_most: Duration) -> bool {
    record(
        name,
        value.as_secs_f64() * 1000.0,
        "ms",
        Expect::AtMost(at_most.as_secs_f64() * 1000.0),
    )
}

/// The median of the samples (the upper one for an even count); zero for none.
pub fn median(samples: &mut [Duration]) -> Duration {
    samples.sort();
    samples.get(samples.len() / 2).copied().unwrap_or_default()
}

fn record_with(
    file: Option<&Path>,
    actions: bool,
    name: &str,
    value: f64,
    unit: &str,
    expect: Expect,
) -> bool {
    let ok = value.is_finite() && expect.holds(value);
    let line = format!(
        "perf: {name} = {} {unit} ({}) {}",
        short(value),
        expect.describe(),
        if ok { "ok" } else { "OUTSIDE" }
    );
    eprintln!("{line}");
    if !ok && actions {
        eprintln!("::warning title=perf::{}", escape_annotation(&line));
    }
    if let Some(path) = file {
        let json = format!(
            "{{\"name\":\"{}\",\"value\":{},\"unit\":\"{}\",\"expect\":{},\"ok\":{ok}}}",
            escape_json(name),
            number(value),
            escape_json(unit),
            expect.json()
        );
        let written = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| writeln!(f, "{json}"));
        if let Err(e) = written {
            eprintln!("perf: could not append to {}: {e}", path.display());
        }
    }
    ok
}

/// A value for a person: three decimals at most, without trailing zeros.
fn short(value: f64) -> String {
    if !value.is_finite() {
        return "n/a".into();
    }
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A JSON number; `null` for NaN and infinity, which JSON cannot hold.
fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "null".into()
    }
}

fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// GitHub's workflow-command escapes for the message of an annotation.
fn escape_annotation(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("twicecat-perf-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        dir
    }

    #[test]
    fn a_floor_holds_at_and_above_it() {
        assert!(Expect::AtLeast(28.0).holds(28.0));
        assert!(Expect::AtLeast(28.0).holds(30.0));
        assert!(!Expect::AtLeast(28.0).holds(27.9));
    }

    #[test]
    fn a_ceiling_holds_at_and_below_it() {
        assert!(Expect::AtMost(16.0).holds(16.0));
        assert!(Expect::AtMost(16.0).holds(4.0));
        assert!(!Expect::AtMost(16.0).holds(16.1));
    }

    #[test]
    fn a_value_with_no_expectation_always_holds() {
        assert!(Expect::Report.holds(0.0));
        assert!(Expect::Report.holds(1e12));
    }

    #[test]
    fn a_value_that_is_not_a_number_is_outside() {
        assert!(!record_with(
            None,
            false,
            "x",
            f64::NAN,
            "fps",
            Expect::Report
        ));
        assert!(!record_with(
            None,
            false,
            "x",
            f64::INFINITY,
            "fps",
            Expect::AtLeast(1.0)
        ));
    }

    #[test]
    fn each_record_appends_one_json_line() {
        let path = temp("append");
        record_with(
            Some(&path),
            false,
            "a.fps",
            27.5,
            "fps",
            Expect::AtLeast(28.0),
        );
        record_with(
            Some(&path),
            false,
            "b.time",
            4.0,
            "ms",
            Expect::AtMost(16.0),
        );
        record_with(Some(&path), false, "c", 1.0, "n", Expect::Report);
        let lines = lines_of(&path);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            r#"{"name":"a.fps","value":27.5,"unit":"fps","expect":{"op":"min","value":28},"ok":false}"#
        );
        assert_eq!(
            lines[1],
            r#"{"name":"b.time","value":4,"unit":"ms","expect":{"op":"max","value":16},"ok":true}"#
        );
        assert_eq!(
            lines[2],
            r#"{"name":"c","value":1,"unit":"n","expect":null,"ok":true}"#
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_name_with_a_quote_or_a_newline_stays_one_json_line() {
        let path = temp("escape");
        record_with(
            Some(&path),
            false,
            "say \"hi\"\nnow\\",
            1.0,
            "n",
            Expect::Report,
        );
        let lines = lines_of(&path);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with(r#"{"name":"say \"hi\"\nnow\\","#),
            "{}",
            lines[0]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_unwritable_file_does_not_panic() {
        let dir = std::env::temp_dir().join(format!("twicecat-perf-{}-nodir", std::process::id()));
        let path = dir.join("no").join("such").join("file.jsonl");
        assert!(record_with(
            Some(&path),
            false,
            "x",
            1.0,
            "n",
            Expect::Report
        ));
    }

    #[test]
    fn a_non_finite_value_is_written_as_null() {
        let path = temp("null");
        record_with(Some(&path), false, "x", f64::NAN, "fps", Expect::Report);
        assert!(lines_of(&path)[0].contains(r#""value":null"#));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_value_is_shown_with_three_decimals_at_most() {
        assert_eq!(short(0.38222700000000004), "0.382");
        assert_eq!(short(30.0), "30");
        assert_eq!(short(27.5), "27.5");
        assert_eq!(short(f64::NAN), "n/a");
    }

    #[test]
    fn annotation_text_escapes_what_github_reads_as_a_command() {
        assert_eq!(escape_annotation("100%\nnext"), "100%25%0Anext");
    }

    #[test]
    fn the_median_is_the_middle_sample() {
        let ms = Duration::from_millis;
        assert_eq!(median(&mut [ms(9), ms(1), ms(5)]), ms(5));
        assert_eq!(median(&mut [ms(4), ms(2)]), ms(4));
        assert_eq!(median(&mut []), Duration::ZERO);
    }

    #[test]
    fn a_duration_is_recorded_in_milliseconds() {
        let path = temp("duration");
        let value = Duration::from_micros(4500);
        let ms = value.as_secs_f64() * 1000.0;
        assert!(record_with(
            Some(&path),
            false,
            "t",
            ms,
            "ms",
            Expect::AtMost(16.0)
        ));
        assert!(lines_of(&path)[0].contains(r#""value":4.5"#));
        let _ = std::fs::remove_file(path);
    }
}
