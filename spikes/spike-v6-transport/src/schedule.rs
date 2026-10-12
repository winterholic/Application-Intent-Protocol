//! Server-configured recurring schedules.
//!
//! A schedule names a durable job and a cron expression evaluated in an IANA time zone.
//! Firing is idempotent: each slot maps to one job key, so several instances and restarts
//! enqueue a slot at most once through the existing job idempotency ledger.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidCron,
    InvalidTimeZone,
    NoSlot,
}

/// Bounded lookback so an impossible expression (for example `0 0 30 2 *`) cannot spin.
const MAX_LOOKBACK_DAYS: i64 = 4 * 366 + 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: [bool; 60],
    hours: [bool; 24],
    days: [bool; 32],
    months: [bool; 13],
    weekdays: [bool; 7],
    day_restricted: bool,
    weekday_restricted: bool,
    hour_restricted: bool,
    zone: Tz,
}

fn field<const N: usize>(text: &str, min: u32, max: u32, names: &[&str]) -> Result<([bool; N], bool), Error> {
    let mut set = [false; N];
    let restricted = text != "*";
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().map_err(|_| Error::InvalidCron)?),
            None => (part, 1),
        };
        if step == 0 {
            return Err(Error::InvalidCron);
        }
        let value = |token: &str| -> Result<u32, Error> {
            if let Some(index) = names.iter().position(|name| name.eq_ignore_ascii_case(token)) {
                return Ok(min + index as u32);
            }
            token.parse::<u32>().map_err(|_| Error::InvalidCron)
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (value(start)?, value(end)?)
        } else {
            let start = value(range)?;
            (start, if part.contains('/') { max } else { start })
        };
        // Cron allows 7 as Sunday; fold it onto 0 before range checks.
        let fold = |n: u32| if N == 7 && n == 7 { 0 } else { n };
        if start > end || start < min || end > max.max(if N == 7 { 7 } else { max }) {
            return Err(Error::InvalidCron);
        }
        let mut n = start;
        while n <= end {
            set[fold(n) as usize] = true;
            n += step;
        }
    }
    Ok((set, restricted))
}

impl Cron {
    /// Parses a five-field expression (`minute hour day month weekday`) or `@hourly`, `@daily`,
    /// `@weekly`, `@monthly`, `@yearly`. Lists, ranges, steps and English month/day names are supported.
    pub fn parse(expression: &str, zone: &str) -> Result<Self, Error> {
        let zone: Tz = zone.parse().map_err(|_| Error::InvalidTimeZone)?;
        let expanded = match expression.trim() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        if expanded.len() > 200 {
            return Err(Error::InvalidCron);
        }
        let parts: Vec<&str> = expanded.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = parts.as_slice() else { return Err(Error::InvalidCron) };
        const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
        const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
        let (minutes, _) = field::<60>(minute, 0, 59, &[])?;
        let (hours, hour_restricted) = field::<24>(hour, 0, 23, &[])?;
        let (days, day_restricted) = field::<32>(day, 1, 31, &[])?;
        let (months, _) = field::<13>(month, 1, 12, &MONTHS)?;
        let (weekdays, weekday_restricted) = field::<7>(weekday, 0, 6, &DAYS)?;
        let cron = Cron { minutes, hours, days, months, weekdays, day_restricted, weekday_restricted, hour_restricted, zone };
        cron.latest_at_or_before(Utc::now()).map(|_| cron).map_err(|_| Error::NoSlot)
    }

    fn date_matches(&self, date: NaiveDate) -> bool {
        if !self.months[date.month() as usize] {
            return false;
        }
        let day = self.days[date.day() as usize];
        let weekday = self.weekdays[date.weekday().num_days_from_sunday() as usize];
        // Vixie cron: when both day fields are restricted, either one may match.
        match (self.day_restricted, self.weekday_restricted) {
            (true, true) => day || weekday,
            (true, false) => day,
            (false, true) => weekday,
            (false, false) => true,
        }
    }

    /// The latest slot at or before `now`. Local times skipped by a DST gap do not fire. In a repeated
    /// hour, fixed-hour schedules fire once (first occurrence) and wildcard-hour schedules fire in both.
    pub fn latest_at_or_before(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>, Error> {
        let local = now.with_timezone(&self.zone).naive_local();
        for back in 0..MAX_LOOKBACK_DAYS {
            let date = local.date() - Duration::days(back);
            if !self.date_matches(date) {
                continue;
            }
            for hour in (0..24u32).rev().filter(|h| self.hours[*h as usize]) {
                for minute in (0..60u32).rev().filter(|m| self.minutes[*m as usize]) {
                    let Some(candidate) = date.and_hms_opt(hour, minute, 0) else { continue };
                    if back == 0 && (candidate.hour(), candidate.minute()) > (local.hour(), local.minute()) {
                        continue;
                    }
                    let instant = match self.zone.from_local_datetime(&candidate) {
                        LocalResult::Single(instant) => instant,
                        LocalResult::Ambiguous(_, second) if !self.hour_restricted && second.with_timezone(&Utc) <= now => second,
                        LocalResult::Ambiguous(first, _) => first,
                        LocalResult::None => continue,
                    };
                    let instant = instant.with_timezone(&Utc);
                    if instant <= now {
                        return Ok(instant);
                    }
                }
            }
        }
        Err(Error::NoSlot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).map(|t| t.with_timezone(&Utc)).expect("test instant")
    }

    #[test]
    fn evaluates_fields_lists_steps_and_names() {
        let cron = Cron::parse("*/15 9-17 * * mon-fri", "UTC").expect("cron");
        assert_eq!(cron.latest_at_or_before(at("2026-10-13T10:07:00Z")), Ok(at("2026-10-13T10:00:00Z")));
        // Saturday falls back to Friday's last business slot.
        assert_eq!(cron.latest_at_or_before(at("2026-10-17T12:00:00Z")), Ok(at("2026-10-16T17:45:00Z")));
        let monthly = Cron::parse("@monthly", "UTC").expect("cron");
        assert_eq!(monthly.latest_at_or_before(at("2026-10-13T10:07:00Z")), Ok(at("2026-10-01T00:00:00Z")));
        let either = Cron::parse("0 0 1 * sun", "UTC").expect("cron");
        // 2026-10-11 is a Sunday; day-or-weekday semantics pick it over the 1st.
        assert_eq!(either.latest_at_or_before(at("2026-10-13T00:00:00Z")), Ok(at("2026-10-11T00:00:00Z")));
        let sunday_seven = Cron::parse("0 0 * * 7", "UTC").expect("cron");
        assert_eq!(sunday_seven.latest_at_or_before(at("2026-10-13T00:00:00Z")), Ok(at("2026-10-11T00:00:00Z")));
    }

    #[test]
    fn uses_the_configured_zone_and_dst_rules() {
        let seoul = Cron::parse("0 3 * * *", "Asia/Seoul").expect("cron");
        assert_eq!(seoul.latest_at_or_before(at("2026-10-13T00:00:00Z")), Ok(at("2026-10-12T18:00:00Z")));
        // 2026-03-08 02:30 does not exist in New York; that day's slot is skipped.
        let gap = Cron::parse("30 2 * * *", "America/New_York").expect("cron");
        assert_eq!(gap.latest_at_or_before(at("2026-03-08T12:00:00Z")), Ok(at("2026-03-07T07:30:00Z")));
        // 2026-11-01 01:30 happens twice; only the first (EDT) occurrence fires.
        let repeat = Cron::parse("30 1 * * *", "America/New_York").expect("cron");
        assert_eq!(repeat.latest_at_or_before(at("2026-11-01T07:00:00Z")), Ok(at("2026-11-01T05:30:00Z")));
        // During the second 01:xx (EST), a wildcard-hour schedule keeps its cadence.
        let every = Cron::parse("*/5 * * * *", "America/New_York").expect("cron");
        assert_eq!(every.latest_at_or_before(at("2026-11-01T06:12:00Z")), Ok(at("2026-11-01T06:10:00Z")));
    }

    #[test]
    fn rejects_malformed_or_impossible_expressions() {
        for bad in ["", "* * * *", "61 * * * *", "*/0 * * * *", "5-1 * * * *", "0 0 * * mon,xyz", "0 0 30 2 *"] {
            assert!(Cron::parse(bad, "UTC").is_err(), "{bad}");
        }
        assert_eq!(Cron::parse("0 0 * * *", "Mars/Olympus"), Err(Error::InvalidTimeZone));
    }
}
