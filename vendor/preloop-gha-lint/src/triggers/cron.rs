//! GitHub Actions `schedule` validation: POSIX five-field cron syntax, the
//! five-minute minimum interval, and the shape of an optional IANA timezone.
//!
//! GitHub documents the accepted operators (`,`, `-`, `*`, `/`), the field
//! ranges, month/day names, and the five-minute minimum:
//! <https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule>
//!
//! The parser is hand-rolled: GitHub rejects six-field (seconds) cron, `@daily`
//! style macros, and Quartz extensions such as `?`, `L`, `W`, and `#`.

use serde_json::Value;

use crate::Diagnostic;

use super::{error, warning};

#[cfg(test)]
const MINUTE_FULL: u64 = (1u64 << 60) - 1;
#[cfg(test)]
const HOUR_FULL: u64 = (1u64 << 24) - 1;
const DOM_FULL: u64 = ((1u64 << 32) - 1) & !1;
#[cfg(test)]
const MONTH_FULL: u64 = ((1u64 << 13) - 1) & !1;
const DOW_FULL: u64 = (1u64 << 7) - 1;

const MONTH_NAMES: &[(&str, u32)] = &[
    ("JAN", 1),
    ("FEB", 2),
    ("MAR", 3),
    ("APR", 4),
    ("MAY", 5),
    ("JUN", 6),
    ("JUL", 7),
    ("AUG", 8),
    ("SEP", 9),
    ("OCT", 10),
    ("NOV", 11),
    ("DEC", 12),
];

const DAY_NAMES: &[(&str, u32)] = &[
    ("SUN", 0),
    ("MON", 1),
    ("TUE", 2),
    ("WED", 3),
    ("THU", 4),
    ("FRI", 5),
    ("SAT", 6),
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cron {
    minute: u64,
    hour: u64,
    day_of_month: u64,
    month: u64,
    day_of_week: u64,
}

/// Validate one `schedule:` list, reporting at `path` (the `schedule` value).
pub(super) fn check_schedule(config: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(schedules) = config.as_array() else {
        return;
    };
    for (index, schedule) in schedules.iter().enumerate() {
        let schedule_path = format!("{path}/{index}");
        let Some(schedule) = schedule.as_object() else {
            findings.push(error(
                "preloop/invalid-cron",
                &schedule_path,
                "schedule entries must be mappings with a `cron` field",
                "write each entry as `- cron: '<expression>'`",
            ));
            continue;
        };
        let cron_path = format!("{schedule_path}/cron");
        match schedule.get("cron") {
            None => findings.push(error(
                "preloop/invalid-cron",
                &cron_path,
                "schedule entry is missing `cron`",
                "add a five-field cron expression, for example `0 4 * * 1`",
            )),
            Some(Value::String(spec)) if spec.contains("${{") => {}
            Some(Value::String(spec)) => match parse(spec) {
                Ok(cron) => {
                    if more_frequent_than_five_minutes(&cron) && has_any_date(&cron) {
                        findings.push(warning(
                            "preloop/cron-too-frequent",
                            &cron_path,
                            format!(
                                "schedule `{spec}` fires more often than GitHub's five-minute minimum"
                            ),
                            "use a step of at least five minutes, for example `*/5 * * * *`",
                        ));
                    }
                }
                Err(reason) => findings.push(error(
                    "preloop/invalid-cron",
                    &cron_path,
                    format!("`{spec}` is not a valid GitHub cron expression: {reason}"),
                    "use five whitespace-separated fields (minute hour day-of-month month day-of-week); GitHub supports `*`, `,`, `-`, `/`, and month/day names",
                )),
            },
            Some(_) => findings.push(error(
                "preloop/invalid-cron",
                &cron_path,
                "`cron` must be a string",
                "write the expression as a quoted string, for example `\"0 4 * * 1\"`",
            )),
        }

        if let Some(timezone) = schedule.get("timezone") {
            let Some(timezone) = timezone.as_str() else {
                continue;
            };
            if !timezone.contains("${{") && !plausible_timezone(timezone) {
                findings.push(error(
                    "preloop/invalid-cron",
                    &format!("{schedule_path}/timezone"),
                    format!("`{timezone}` is not a plausible IANA timezone name"),
                    "use `UTC` or an `Area/Location` name such as `America/New_York`",
                ));
            }
        }
    }
}

fn parse(spec: &str) -> Result<Cron, String> {
    let fields: Vec<&str> = spec.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!("expected 5 fields, found {}", fields.len()));
    }
    Ok(Cron {
        minute: parse_field(fields[0], 0, 59, None, "minute")?,
        hour: parse_field(fields[1], 0, 23, None, "hour")?,
        day_of_month: parse_field(fields[2], 1, 31, None, "day-of-month")?,
        month: parse_field(fields[3], 1, 12, Some(MONTH_NAMES), "month")?,
        day_of_week: parse_field(fields[4], 0, 6, Some(DAY_NAMES), "day-of-week")?,
    })
}

fn parse_field(
    field: &str,
    min: u32,
    max: u32,
    names: Option<&[(&str, u32)]>,
    label: &str,
) -> Result<u64, String> {
    let mut mask = 0u64;
    for part in field.split(',') {
        if part.is_empty() {
            return Err(format!("{label} field has an empty list item"));
        }
        let (base, step) = match part.split_once('/') {
            Some((base, step)) => {
                if base.is_empty() {
                    return Err(format!("{label} field is missing a value before `/`"));
                }
                let step: u64 = step
                    .parse()
                    .map_err(|_| format!("{label} field has invalid step `{step}`"))?;
                if step == 0 {
                    return Err(format!("{label} field step must be greater than zero"));
                }
                (base, Some(step))
            }
            None => (part, None),
        };

        let (start, end) = if base == "*" {
            (min, max)
        } else if let Some((start, end)) = base.split_once('-') {
            if start.is_empty() || end.is_empty() {
                return Err(format!(
                    "{label} field range `{base}` is missing an endpoint"
                ));
            }
            let start = parse_value(start, min, max, names, label)?;
            let end = parse_value(end, min, max, names, label)?;
            if start > end {
                return Err(format!("{label} field range `{base}` is reversed"));
            }
            (start, end)
        } else {
            let value = parse_value(base, min, max, names, label)?;
            match step {
                // Vixie-style `N/step` runs from N through the field maximum.
                Some(_) => (value, max),
                None => (value, value),
            }
        };

        let step = step.unwrap_or(1);
        let mut value = u64::from(start);
        let end = u64::from(end);
        while value <= end {
            mask |= 1u64 << value;
            value = match value.checked_add(step) {
                Some(next) => next,
                None => break,
            };
        }
    }
    Ok(mask)
}

fn parse_value(
    text: &str,
    min: u32,
    max: u32,
    names: Option<&[(&str, u32)]>,
    label: &str,
) -> Result<u32, String> {
    if let Some(names) = names
        && let Some((_, value)) = names
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(text))
    {
        return Ok(*value);
    }
    let value: u32 = text
        .parse()
        .map_err(|_| format!("{label} field has invalid value `{text}`"))?;
    if !(min..=max).contains(&value) {
        return Err(format!(
            "{label} field value {value} is outside {min}-{max}"
        ));
    }
    Ok(value)
}

/// True when the hour/minute pattern produces two runs less than five minutes
/// apart within a firing day. Day-of-week and day-of-month restrictions only
/// reduce the set of firing days, never the spacing within one.
fn more_frequent_than_five_minutes(cron: &Cron) -> bool {
    let mut minutes = Vec::new();
    for hour in 0..24u32 {
        if cron.hour & (1u64 << hour) == 0 {
            continue;
        }
        for minute in 0..60u32 {
            if cron.minute & (1u64 << minute) != 0 {
                minutes.push(hour * 60 + minute);
            }
        }
    }
    if minutes.len() < 2 {
        return false;
    }
    let mut smallest = u32::MAX;
    for pair in minutes.windows(2) {
        smallest = smallest.min(pair[1] - pair[0]);
    }
    let wrap = 1440 + minutes[0] - minutes[minutes.len() - 1];
    smallest.min(wrap) < 5
}

/// Whether any calendar date can satisfy the day/month masks. This keeps the
/// frequency warning off schedules such as `* * 31 2 *` that never fire.
fn has_any_date(cron: &Cron) -> bool {
    let dom_restricted = cron.day_of_month != DOM_FULL;
    let dow_restricted = cron.day_of_week != DOW_FULL;
    for month in 1..=12u32 {
        if cron.month & (1u64 << month) == 0 {
            continue;
        }
        if dom_restricted && !dow_restricted {
            let matches = (1..=31u32)
                .any(|day| cron.day_of_month & (1u64 << day) != 0 && month_has_day(month, day));
            if matches {
                return true;
            }
        } else {
            // With both day fields restricted, cron's OR semantics and the
            // weekday cycle guarantee a match; with only the weekday field
            // restricted, every month has all seven weekdays.
            return true;
        }
    }
    false
}

fn month_has_day(month: u32, day: u32) -> bool {
    let longest = match month {
        2 => 29, // leap years exist in every 400-year cycle
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    day <= longest
}

/// Shape-only IANA timezone check. `UTC` is accepted explicitly; everything
/// else must look like `Area/Location` (optionally `Area/Region/Location`).
/// No timezone database is shipped.
fn plausible_timezone(timezone: &str) -> bool {
    if timezone.eq_ignore_ascii_case("utc") {
        return true;
    }
    let segments: Vec<&str> = timezone.split('/').collect();
    if !(2..=3).contains(&segments.len()) {
        return false;
    }
    segments.iter().all(|segment| {
        let mut characters = segment.chars();
        matches!(characters.next(), Some(first) if first.is_ascii_alphabetic())
            && characters.all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '+')
            })
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Cron, DOM_FULL, DOW_FULL, HOUR_FULL, MINUTE_FULL, MONTH_FULL,
        more_frequent_than_five_minutes, parse, plausible_timezone,
    };

    #[test]
    fn accepts_standard_five_field_expressions() {
        for spec in [
            "0 4 * * 1",
            "*/15 * * * *",
            "0 0 1 JAN *",
            "5-10/2 * * * *",
            "0 0 * * SUN",
            "0,30 * * * *",
            "0 0 29 2 *",
            "0 0 1-5/2 * MON-FRI",
            "45 3 * * *",
            "0 0 * * 0",
        ] {
            assert!(parse(spec).is_ok(), "{spec}: {:?}", parse(spec));
        }
    }

    #[test]
    fn rejects_non_github_cron_syntax() {
        for spec in [
            "",
            "0 0 * *",
            "0 0 * * * *",
            "@daily",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * 32 * *",
            "* * * 13 *",
            "* * * * 7",
            "*/0 * * * *",
            "5-1 * * * *",
            "* * * FOO *",
            "* * * * MONDAY",
            "? * * * *",
            "0 0 L * *",
            "1-2-3 * * * *",
            "1,,2 * * * *",
        ] {
            assert!(parse(spec).is_err(), "{spec} should be rejected");
        }
    }

    #[test]
    fn flags_schedules_below_the_five_minute_floor() {
        for spec in ["* * * * *", "*/2 * * * *", "0,1 * * * *", "59,0 * * * *"] {
            let cron = parse(spec).expect(spec);
            assert!(more_frequent_than_five_minutes(&cron), "{spec}");
        }
        for spec in [
            "*/5 * * * *",
            "0 * * * *",
            "0 0 * * *",
            "1-59/5 * * * *",
            "0,5,10 * * * *",
        ] {
            let cron = parse(spec).expect(spec);
            assert!(!more_frequent_than_five_minutes(&cron), "{spec}");
        }
    }

    #[test]
    fn full_masks_cover_their_domains() {
        let full = parse("* * * * *").expect("parse");
        assert_eq!(full.minute, MINUTE_FULL);
        assert_eq!(full.hour, HOUR_FULL);
        assert_eq!(full.day_of_month, DOM_FULL);
        assert_eq!(full.month, MONTH_FULL);
        assert_eq!(full.day_of_week, DOW_FULL);
    }

    #[test]
    fn impossible_dates_are_not_frequent() {
        let cron: Cron = parse("* * 31 2 *").expect("parse");
        assert!(!super::has_any_date(&cron));
    }

    #[test]
    fn timezone_shape_is_plausible() {
        for timezone in [
            "UTC",
            "utc",
            "America/New_York",
            "Etc/GMT+5",
            "America/Argentina/Buenos_Aires",
            "Europe/London",
            "Asia/Kolkata",
        ] {
            assert!(plausible_timezone(timezone), "{timezone}");
        }
        for timezone in [
            "",
            "Local",
            "America",
            "America/",
            "/New_York",
            "Europe/London/Extra/More",
            "America/New York",
            "America/New_York!",
            "GMT+5",
        ] {
            assert!(!plausible_timezone(timezone), "{timezone}");
        }
    }
}
