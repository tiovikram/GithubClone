//! POSIX cron expressions for `on: schedule` (five fields, UTC): minute hour day-of-month month
//! day-of-week, with `*`, lists, ranges, steps and month/day names.
use chrono::{DateTime, Datelike, Timelike, Utc};

#[derive(Debug, Clone)]
pub struct Cron {
    minute: Vec<bool>,
    hour: Vec<bool>,
    dom: Vec<bool>,
    month: Vec<bool>,
    dow: Vec<bool>,
    dom_star: bool,
    dow_star: bool,
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn value(s: &str, names: &[&str], offset: u32) -> Option<u32> {
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    let l = s.to_ascii_lowercase();
    names.iter().position(|n| *n == l).map(|p| p as u32 + offset)
}

fn field(spec: &str, min: u32, max: u32, names: &[&str], offset: u32) -> Option<Vec<bool>> {
    let mut set = vec![false; (max + 1) as usize];
    for part in spec.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (value(a, names, offset)?, value(b, names, offset)?)
        } else {
            let v = value(range, names, offset)?;
            (v, if part.contains('/') { max } else { v })
        };
        // day-of-week 7 is Sunday too
        let hi = if max == 6 && hi == 7 { 7 } else { hi };
        if lo < min || hi > max.max(if max == 6 { 7 } else { max }) || lo > hi {
            return None;
        }
        let mut v = lo;
        while v <= hi {
            set[(if max == 6 { v % 7 } else { v }) as usize] = true;
            v += step;
        }
    }
    Some(set)
}

impl Cron {
    pub fn parse(expr: &str) -> Option<Cron> {
        let f: Vec<&str> = expr.split_whitespace().collect();
        if f.len() != 5 {
            return None;
        }
        Some(Cron {
            minute: field(f[0], 0, 59, &[], 0)?,
            hour: field(f[1], 0, 23, &[], 0)?,
            dom: field(f[2], 1, 31, &[], 0)?,
            month: field(f[3], 1, 12, &MONTHS, 1)?,
            dow: field(f[4], 0, 6, &DAYS, 0)?,
            dom_star: f[2] == "*",
            dow_star: f[4] == "*",
        })
    }

    pub fn matches(&self, t: DateTime<Utc>) -> bool {
        if !self.minute[t.minute() as usize] || !self.hour[t.hour() as usize] || !self.month[t.month() as usize] {
            return false;
        }
        let dom = self.dom[t.day() as usize];
        let dow = self.dow[t.weekday().num_days_from_sunday() as usize];
        // POSIX: when both day fields are restricted, either may match.
        match (self.dom_star, self.dow_star) {
            (true, true) => true,
            (true, false) => dow,
            (false, true) => dom,
            (false, false) => dom || dow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn cron() {
        let t = Utc.with_ymd_and_hms(2026, 10, 17, 6, 30, 0).unwrap(); // a Saturday
        assert!(Cron::parse("* * * * *").unwrap().matches(t));
        assert!(Cron::parse("*/15 * * * *").unwrap().matches(t));
        assert!(!Cron::parse("*/20 * * * *").unwrap().matches(t));
        assert!(Cron::parse("30 6 * * sat").unwrap().matches(t));
        assert!(Cron::parse("30 6 * * 1-6").unwrap().matches(t));
        assert!(!Cron::parse("30 6 * * MON-FRI").unwrap().matches(t));
        assert!(Cron::parse("0,30 5-7 17 OCT *").unwrap().matches(t));
        assert!(Cron::parse("bad").is_none());
        assert!(Cron::parse("61 * * * *").is_none());
    }
}
