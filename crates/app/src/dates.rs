//! Day and time labels for the timeline (plan Q21).
//!
//! Both functions are generic over the time zone and take "now" as an
//! argument, so tests use fixed offsets and a fixed clock. English only.

use chrono::{DateTime, Datelike, TimeZone, Utc};

/// Shown when a timestamp is outside the range chrono can represent.
pub const UNKNOWN_DATE: &str = "Unknown date";

fn at<Tz: TimeZone>(ms: u64, tz: &Tz) -> Option<DateTime<Tz>> {
    let ms = i64::try_from(ms).ok()?;
    Some(DateTime::<Utc>::from_timestamp_millis(ms)?.with_timezone(tz))
}

/// The label of the day of `ts_ms` (milliseconds since the Unix epoch), as
/// seen from `now_ms` in `tz`: "Today", "Yesterday", a weekday name within
/// the last six days, `2 October` in the current year, `2 October 2025`
/// otherwise (also for days in the future).
pub fn day_label<Tz: TimeZone>(ts_ms: u64, now_ms: u64, tz: &Tz) -> String {
    let (Some(ts), Some(now)) = (at(ts_ms, tz), at(now_ms, tz)) else {
        return UNKNOWN_DATE.to_owned();
    };
    let (day, today) = (ts.date_naive(), now.date_naive());
    match today.signed_duration_since(day).num_days() {
        0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        2..=6 => ts.naive_local().format("%A").to_string(),
        _ if day.year() == today.year() => ts.naive_local().format("%-d %B").to_string(),
        _ => ts.naive_local().format("%-d %B %Y").to_string(),
    }
}

/// The time of day of `ts_ms` in `tz` as `HH:MM`.
pub fn time_label<Tz: TimeZone>(ts_ms: u64, tz: &Tz) -> String {
    at(ts_ms, tz).map_or_else(String::new, |t| t.naive_local().format("%H:%M").to_string())
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn ms(tz: &FixedOffset, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> u64 {
        let t = tz
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid local time");
        u64::try_from(t.timestamp_millis()).expect("after 1970")
    }

    fn zones() -> [FixedOffset; 3] {
        [
            FixedOffset::east_opt(14 * 3600).expect("+14:00"),
            FixedOffset::west_opt(12 * 3600).expect("-12:00"),
            FixedOffset::east_opt(0).expect("UTC"),
        ]
    }

    #[test]
    fn today_and_yesterday_across_midnight() {
        for tz in zones() {
            let now = ms(&tz, 2026, 10, 2, 0, 5);
            assert_eq!(day_label(ms(&tz, 2026, 10, 2, 0, 0), now, &tz), "Today");
            assert_eq!(
                day_label(ms(&tz, 2026, 10, 1, 23, 59), now, &tz),
                "Yesterday"
            );
            assert_eq!(day_label(ms(&tz, 2026, 10, 1, 0, 0), now, &tz), "Yesterday");
        }
    }

    #[test]
    fn the_same_instant_is_a_different_day_in_different_zones() {
        let east = FixedOffset::east_opt(14 * 3600).expect("+14:00");
        let west = FixedOffset::west_opt(12 * 3600).expect("-12:00");
        // 2 Oct 00:00 in +14 is 1 Oct 10:00 UTC, which is 1 Oct 22:00 in -12.
        let now = ms(&east, 2026, 10, 2, 0, 0);
        assert_eq!(day_label(now, now, &east), "Today");
        assert_eq!(day_label(now, now, &west), "Today");
        let earlier = now - 2 * 3600 * 1000;
        assert_eq!(day_label(earlier, now, &east), "Yesterday");
        assert_eq!(day_label(earlier, now, &west), "Today");
    }

    #[test]
    fn weekday_names_for_the_last_six_days() {
        let tz = FixedOffset::east_opt(0).expect("UTC");
        // 2026-10-02 is a Friday.
        let now = ms(&tz, 2026, 10, 2, 12, 0);
        assert_eq!(day_label(ms(&tz, 2026, 9, 30, 9, 0), now, &tz), "Wednesday");
        assert_eq!(day_label(ms(&tz, 2026, 9, 26, 9, 0), now, &tz), "Saturday");
        // Seven days back is a date.
        assert_eq!(
            day_label(ms(&tz, 2026, 9, 25, 9, 0), now, &tz),
            "25 September"
        );
    }

    #[test]
    fn dates_in_the_current_and_other_years() {
        for tz in zones() {
            let now = ms(&tz, 2026, 1, 10, 12, 0);
            assert_eq!(day_label(ms(&tz, 2026, 1, 1, 0, 0), now, &tz), "1 January");
            let now = ms(&tz, 2026, 1, 3, 12, 0);
            assert_eq!(
                day_label(ms(&tz, 2025, 12, 31, 23, 59), now, &tz),
                "Wednesday"
            );
            assert_eq!(
                day_label(ms(&tz, 2025, 10, 2, 8, 0), now, &tz),
                "2 October 2025"
            );
        }
    }

    #[test]
    fn year_boundary_yesterday() {
        for tz in zones() {
            let now = ms(&tz, 2026, 1, 1, 0, 1);
            assert_eq!(
                day_label(ms(&tz, 2025, 12, 31, 23, 59), now, &tz),
                "Yesterday"
            );
        }
    }

    #[test]
    fn future_days_show_a_date() {
        let tz = FixedOffset::east_opt(0).expect("UTC");
        let now = ms(&tz, 2026, 10, 2, 12, 0);
        assert_eq!(day_label(ms(&tz, 2026, 10, 9, 9, 0), now, &tz), "9 October");
        assert_eq!(
            day_label(ms(&tz, 2027, 1, 9, 9, 0), now, &tz),
            "9 January 2027"
        );
    }

    #[test]
    fn time_labels_follow_the_zone() {
        let east = FixedOffset::east_opt(14 * 3600).expect("+14:00");
        let west = FixedOffset::west_opt(12 * 3600).expect("-12:00");
        let t = ms(&east, 2026, 10, 2, 0, 5);
        assert_eq!(time_label(t, &east), "00:05");
        assert_eq!(time_label(t, &west), "22:05");
        assert_eq!(time_label(ms(&east, 2026, 10, 2, 23, 59), &east), "23:59");
    }

    #[test]
    fn out_of_range_timestamps_do_not_panic() {
        let tz = FixedOffset::east_opt(0).expect("UTC");
        assert_eq!(day_label(u64::MAX, 0, &tz), UNKNOWN_DATE);
        assert_eq!(day_label(0, u64::MAX, &tz), UNKNOWN_DATE);
        assert_eq!(time_label(u64::MAX, &tz), "");
    }
}
