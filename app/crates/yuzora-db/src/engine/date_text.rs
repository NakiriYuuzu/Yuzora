//! What a text typed into a SQL Server `datetime` or `smalldatetime` cell becomes on its way to the server.
//!
//! These two types read a date from text by the session's `DATEFORMAT`, which the login's
//! language sets: a day-first login stores `2024-01-06` as the 1st of June, whatever order
//! the person wrote the date and the time in. So the question is never which spellings are
//! known. It is whether a year-first date is in the text at all, and then the server is not
//! asked to read it.
//!
//! Every text falls in exactly one row of this table, and only the second sends something
//! the server reads in more than one way:
//!
//! | The text | What is done | Why |
//! | --- | --- | --- |
//! | has no year-first date in it (`1/6/2024`, `Jan 6 2024`, `10:00`, `2024 Jan 6`, `240106`) | **Session**: sent as typed | a person's own writing of the date is read by the language they log in with; a month's name or a year with no day has one reading under every one (checked against the six `DATEFORMAT`s) |
//! | has a year-first date in it, and is nothing but that date, a time before or after it, and a zone after the time | **Client**: sent as `YYYY-MM-DDThh:mm:ss[.f][zone]` | that form is read the same under every `DATEFORMAT` and language; the zone is the server's to accept or refuse (`datetime` takes `Z`, and has no place for an offset) |
//! | has a year-first date in it, and is anything else | **Refused**, before a statement is sent | no reading of it is certain |
//!
//! A year-first date is four digits that no digit touches, then optionally spaces and one of
//! `-` `/` `.` and a digit (the server reads `2024- 01- 06` too, and a day-first login the
//! wrong way), or eight digits (`20240106`). Spaces, a stray character, a second date, a
//! one-digit minute, a colon before the fraction, a month's name and a bare year are all in
//! the third row once a date of that kind is in the text.

use chrono::NaiveDate;

pub(super) enum DateText {
    Session,
    /// The text, written the one way every session reads.
    Client(String),
    Refused,
}

pub(super) fn classify(text: &str) -> DateText {
    let text = text.trim();
    match year_first_at(text) {
        None => DateText::Session,
        Some(start) => read(text, start).map_or(DateText::Refused, DateText::Client),
    }
}

/// Where the first year-first date starts.
fn year_first_at(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let spaces = |mut at: usize| {
        while b.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    };
    let mut at = 0;
    while at < b.len() {
        if !b[at].is_ascii_digit() || (at > 0 && b[at - 1].is_ascii_digit()) {
            at += 1;
            continue;
        }
        let digits = digits_from(b, at);
        if digits == 8 {
            return Some(at);
        }
        if digits == 4 {
            let separator = spaces(at + 4);
            if matches!(b.get(separator), Some(b'-' | b'/' | b'.')) && b.get(spaces(separator + 1)).is_some_and(u8::is_ascii_digit) {
                return Some(at);
            }
        }
        at += digits;
    }
    None
}

fn digits_from(b: &[u8], at: usize) -> usize {
    b[at..].iter().take_while(|c| c.is_ascii_digit()).count()
}

/// The text as `YYYY-MM-DDThh:mm:ss[.f][zone]`, if it is exactly a date and a time.
fn read(text: &str, start: usize) -> Option<String> {
    let ((year, month, day), end) = date_at(text, start)?;
    let (before, after) = (&text[..start], &text[end..]);
    let time = match (before.is_empty(), after.is_empty()) {
        (true, true) => "00:00:00".to_owned(),
        // `T` joins the time to the date; without it a space does.
        (true, false) => clock(match after.strip_prefix('T') {
            Some(time) => time,
            None if after.starts_with(char::is_whitespace) => after.trim_start(),
            None => return None,
        })?,
        (false, true) if before.ends_with(char::is_whitespace) => clock(before.trim_end())?,
        _ => return None,
    };
    Some(format!("{year:04}-{month:02}-{day:02}T{time}"))
}

/// `2024-01-06`, `2024/1/6`, `2024.01.06` (one separator, twice) or `20240106` at `start`, and
/// where it ends. Only a real day of the calendar.
fn date_at(text: &str, start: usize) -> Option<((i32, u32, u32), usize)> {
    let b = text.as_bytes();
    let (year, month, day, end) = match digits_from(b, start) {
        8 => (&text[start..start + 4], &text[start + 4..start + 6], &text[start + 6..start + 8], start + 8),
        4 => {
            let separator = *b.get(start + 4).filter(|c| matches!(c, b'-' | b'/' | b'.'))?;
            let month_at = start + 5;
            let month_len = digits_from(b, month_at);
            let day_at = month_at + month_len + 1;
            let day_len = if b.get(day_at - 1) == Some(&separator) { digits_from(b, day_at) } else { 0 };
            if !(1..=2).contains(&month_len) || !(1..=2).contains(&day_len) {
                return None;
            }
            (&text[start..start + 4], &text[month_at..month_at + month_len], &text[day_at..day_at + day_len], day_at + day_len)
        }
        _ => return None,
    };
    let (year, month, day): (i32, u32, u32) = (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    NaiveDate::from_ymd_opt(year, month, day)?;
    Some(((year, month, day), end))
}

/// `14:30`, `14:30:15.5`, `2:30 PM`, `10AM`, `10:00:00Z` as `HH:MM:SS[.f][zone]`. The minutes
/// and seconds are two digits; a colon before the fraction (SQL Server reads the digits after
/// it as milliseconds, not as a fraction), a zone that is not stuck to the time, a zone after
/// AM or PM, and a clock that is neither 24-hour nor 12-hour with AM or PM are not read.
fn clock(text: &str) -> Option<String> {
    let (text, zone) = split_zone(text);
    let (time, pm) = match text.len().checked_sub(2).and_then(|at| text.get(at..)) {
        Some(tail) if tail.eq_ignore_ascii_case("am") => (text[..text.len() - 2].trim_end(), Some(false)),
        Some(tail) if tail.eq_ignore_ascii_case("pm") => (text[..text.len() - 2].trim_end(), Some(true)),
        _ => (text, None),
    };
    if zone.is_some() && pm.is_some() {
        return None;
    }
    let (time, fraction) = match time.split_once('.') {
        Some((time, fraction)) => (time, Some(fraction)),
        None => (time, None),
    };
    let mut fields = time.split(':');
    let hour = number(fields.next()?, 1..=2)?;
    let minute = match fields.next() {
        Some(m) => Some(number(m, 2..=2)?),
        None => None,
    };
    let second = match fields.next() {
        Some(s) => Some(number(s, 2..=2)?),
        None => None,
    };
    if fields.next().is_some() || (fraction.is_some() && second.is_none()) {
        return None;
    }
    let fraction = match fraction {
        Some(f) if (1..=9).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()) => format!(".{f}"),
        Some(_) => return None,
        None => String::new(),
    };
    let hour = match pm {
        Some(pm) if (1..=12).contains(&hour) => hour % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if hour <= 23 && minute.is_some() => hour,
        None => return None,
    };
    let (minute, second) = (minute.unwrap_or(0), second.unwrap_or(0));
    if minute > 59 || second > 59 {
        return None;
    }
    Some(format!("{hour:02}:{minute:02}:{second:02}{fraction}{}", zone.unwrap_or_default()))
}

/// `Z` or `+08:00` / `-05:30` stuck to the last digit of a time, and the time without it.
fn split_zone(text: &str) -> (&str, Option<String>) {
    let stuck = |body: &str| body.ends_with(|c: char| c.is_ascii_digit());
    if let Some(body) = text.strip_suffix(['Z', 'z']).filter(|body| stuck(body)) {
        return (body, Some("Z".to_owned()));
    }
    if let Some(at) = text.len().checked_sub(6).filter(|at| text.is_char_boundary(*at)) {
        let (body, tail) = text.split_at(at);
        let t = tail.as_bytes();
        if matches!(t[0], b'+' | b'-') && t[1].is_ascii_digit() && t[2].is_ascii_digit() && t[3] == b':' && t[4].is_ascii_digit() && t[5].is_ascii_digit() && stuck(body) {
            return (body, Some(tail.to_owned()));
        }
    }
    (text, None)
}

fn number(part: &str, digits: std::ops::RangeInclusive<usize>) -> Option<u32> {
    (digits.contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(text: &str) -> Option<String> {
        match classify(text) {
            DateText::Client(iso) => Some(iso),
            _ => None,
        }
    }

    fn is_session(text: &str) -> bool {
        matches!(classify(text), DateText::Session)
    }

    #[test]
    fn a_year_first_date_is_written_the_one_way_every_dateformat_reads() {
        for (typed, sent) in [
            ("2024-01-06 10:00:00", "2024-01-06T10:00:00"),
            ("2024-01-13 10:00:00.997", "2024-01-13T10:00:00.997"),
            ("2024-01-06T10:00", "2024-01-06T10:00:00"),
            ("2024-01-06", "2024-01-06T00:00:00"),
            ("2024/1/6 9:05", "2024-01-06T09:05:00"),
            ("  2024.01.06 10:00:00  ", "2024-01-06T10:00:00"),
            ("20240106", "2024-01-06T00:00:00"),
            ("20240106 10:00", "2024-01-06T10:00:00"),
            ("20240106T10:00:00", "2024-01-06T10:00:00"),
            ("2024-01-06 2:30 PM", "2024-01-06T14:30:00"),
            ("2024-01-06 2:30PM", "2024-01-06T14:30:00"),
            ("2024-01-06 02:30 pm", "2024-01-06T14:30:00"),
            ("2024-01-06 10:00:00 AM", "2024-01-06T10:00:00"),
            ("2024-01-06 12:00 AM", "2024-01-06T00:00:00"),
            ("2024-01-06 12:30 AM", "2024-01-06T00:30:00"),
            ("2024-01-06 12:00 PM", "2024-01-06T12:00:00"),
            ("2024-01-06 12:30:15.5 PM", "2024-01-06T12:30:15.5"),
            ("2024-01-06 10AM", "2024-01-06T10:00:00"),
            ("2024-01-06 11 pm", "2024-01-06T23:00:00"),
        ] {
            assert_eq!(client(typed).as_deref(), Some(sent), "{typed}");
        }
    }

    #[test]
    fn the_time_may_come_before_the_date() {
        for (typed, sent) in [
            ("10:00 2024-01-06", "2024-01-06T10:00:00"),
            ("2:30 PM 2024/1/6", "2024-01-06T14:30:00"),
            ("10:00:00 2024.01.06", "2024-01-06T10:00:00"),
            ("10:00:00.5  20240106", "2024-01-06T10:00:00.5"),
            ("12:00 AM 2024-01-06", "2024-01-06T00:00:00"),
            ("10:00:00Z 2024-01-06", "2024-01-06T10:00:00Z"),
        ] {
            assert_eq!(client(typed).as_deref(), Some(sent), "{typed}");
        }
        for typed in ["10:00 2024-01-06 11:00", "10:00T2024-01-06", "10:00:5 2024-01-06", "2024-01-06 2024-01-07"] {
            assert!(matches!(classify(typed), DateText::Refused), "{typed}");
        }
    }

    #[test]
    fn iso_8601_is_taken_with_a_z_and_a_fraction_and_leaves_an_offset_to_the_server() {
        for (typed, sent) in [
            ("2024-01-06T10:00:00Z", "2024-01-06T10:00:00Z"),
            ("2024-01-06T10:00:00z", "2024-01-06T10:00:00Z"),
            ("2024-01-06T10:00:00.000Z", "2024-01-06T10:00:00.000Z"),
            ("2024-01-06T10:00:00.25Z", "2024-01-06T10:00:00.25Z"),
            ("2024-01-06 10:00:00Z", "2024-01-06T10:00:00Z"),
            ("2024-01-06T10:00Z", "2024-01-06T10:00:00Z"),
            ("2024-01-06T10:00:00+08:00", "2024-01-06T10:00:00+08:00"),
            ("2024-01-06T10:00:00-05:30", "2024-01-06T10:00:00-05:30"),
            ("2024-01-06T10:00:00.5+00:00", "2024-01-06T10:00:00.5+00:00"),
        ] {
            assert_eq!(client(typed).as_deref(), Some(sent), "{typed}");
        }
        for typed in [
            "2024-01-06T10:00:00 Z",
            "2024-01-06T10:00:00 +08:00",
            "2024-01-06T10:00:00+0800",
            "2024-01-06T10:00:00+08",
            "2024-01-06 2:30 PM Z",
            "2024-01-06 10 PMZ",
            "2024-01-06Z",
            "2024-01-06T10:00:00Z Z",
        ] {
            assert!(matches!(classify(typed), DateText::Refused), "{typed}");
        }
    }

    #[test]
    fn what_is_not_exactly_a_date_and_a_time_is_refused() {
        for typed in [
            "2024-01-06 9:5",
            "2024-01-06 10:00:5",
            "2024-01-06 1:2:3",
            "2024-01-06 10",
            "2024-01-06 10:00:00:000",
            "2024-01-06 10:00:00:5",
            "2024-01-06 10:00 :00",
            "2024-01-06 13:00 PM",
            "2024-01-06 0:30 AM",
            "2024-01-06 24:00",
            "2024-01-06 10:60",
            "2024-01-06 10:00:60",
            "2024-01-06 10:00:00.",
            "2024-01-06 10:00:00.1234567890",
            "2024-02-30",
            "2024-13-06",
            "2024-00-06",
            "2024-01-06-07",
            "2024-01/06",
            "2024/01.06",
            "2024-01",
            "2024/1",
            "2024-1-6-",
            "2024-01-06.",
            "2024-01-06x",
            "2024-01-0610:00",
            "x2024-01-06",
            "+2024-01-06",
            "(2024-01-06)",
            "'2024-01-06'",
            "{ts '2024-01-06 10:00:00'}",
            "2024- 01- 06",
            "2024 -01 -06",
            "2024 - 01 - 06",
            "2024/ 1/ 6",
            "2024. 01. 06",
            "2024\t-01-06",
            "10:00 2024- 01- 06",
            "2024-01-06T",
            "2024-01-06 T10:00",
            "2024-01-06T 10:00",
            "2024-01-06 AM",
        ] {
            assert!(matches!(classify(typed), DateText::Refused), "{typed:?} has a year-first date in it, and is not just that and a time");
        }
    }

    #[test]
    fn a_text_with_no_year_first_date_in_it_is_the_sessions() {
        for typed in [
            "",
            "   ",
            "10:00",
            "10:00:00.5",
            "2:30 PM",
            "13/01/2024",
            "1/6/2024",
            "01-06-2024",
            "6.1.2024",
            "1/6/24",
            "24-01-06",
            "06/01/24 10:00",
            "10:00 06/01/2024",
            "Jan 6 2024",
            "6 Jan 2024",
            "2024 Jan 6",
            "2024-Jan-06",
            "2024 January 6",
            "2024",
            "240106",
            "202401",
            "2024010",
            "202401061030",
            "1030",
            "x",
            "tomorrow",
            "2024--01--06",
        ] {
            assert!(is_session(typed), "{typed:?}");
        }
    }

    #[test]
    fn every_year_first_date_in_every_order_is_the_clients_or_refused_and_never_the_sessions() {
        let clocks = ["", "10:00", "10:00:00.5", "2:30 PM", "12:00 AM", "9:5", "10:00:00:5", "13:00 PM", "10:00:00Z", "10:00:00+08:00", "x", "10 : 00"];
        let mut count = 0;
        for (y, m, d) in [(2024, 1, 6), (2024, 2, 3), (2024, 1, 13), (2024, 12, 11), (2023, 2, 29), (2024, 13, 6)] {
            for date in [
                format!("{y}-{m:02}-{d:02}"),
                format!("{y}-{m}-{d}"),
                format!("{y}/{m}/{d}"),
                format!("{y}.{m:02}.{d:02}"),
                format!("{y}{m:02}{d:02}"),
                format!("{y}- {m:02}-{d:02}"),
                format!("{y} /{m}/{d}"),
                format!("{y}-{m:02}/{d:02}"),
                format!("{y}-{m:02}"),
            ] {
                for clock in clocks {
                    for typed in [
                        format!("{date} {clock}"),
                        format!("{date}T{clock}"),
                        format!("{date}  {clock}"),
                        format!("{clock} {date}"),
                        format!("  {clock}  {date} "),
                    ] {
                        count += 1;
                        assert!(!is_session(&typed), "{typed:?} has a year-first date and went to the session");
                        // Whatever the shape, what is sent has the date that was typed.
                        if let Some(sent) = client(&typed) {
                            assert!(sent.starts_with(&format!("{y:04}-{m:02}-{d:02}T")), "{typed:?} was sent as {sent}");
                        }
                    }
                }
            }
        }
        assert!(count > 3000, "{count}");
    }
}
