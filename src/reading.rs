//! Strict regex reading of one status message into absences.
//!
//! The reading accepts a message only when it finds one kind and understands every date word.
//! Anything else comes back as a list of doubts for manual review.

use std::fmt;
use std::sync::LazyLock;

use jiff::ToSpan;
use jiff::civil::{Date, Weekday};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// The class of an absence, named by the lab's letter codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// Working from home.
    #[serde(rename = "WFH")]
    Remote,
    /// Sick day.
    #[serde(rename = "SIC")]
    Sick,
    /// Vacation day.
    #[serde(rename = "VAC")]
    Vacation,
    /// Personal day.
    #[serde(rename = "PERS")]
    Personal,
    /// Time off with no stated code, travel or a conference.
    #[serde(rename = "OUT")]
    Out,
}

/// The part of each day an absence covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    /// The whole day.
    #[default]
    Full,
    /// The morning only.
    Morning,
    /// The afternoon only.
    Afternoon,
    /// Half of the day, with no word on which half.
    Half,
}

/// One kind of absence over an inclusive span of days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Absence {
    /// The class of the absence.
    pub kind: Kind,
    /// The first day away.
    pub first: Date,
    /// The last day away.
    pub last: Date,
    /// The part of each day away.
    #[serde(default)]
    pub part: Part,
}

/// A reason the regex reading refused a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Doubt {
    /// No kind keyword matched.
    NoKind,
    /// More than one coded kind matched.
    TwoKinds,
    /// Date-like words remained that no pattern understood.
    UnreadDateWords(String),
    /// A weekday name does not fall on any date the message gives.
    WeekdayDisagrees,
    /// `today` or `tomorrow` does not fall on any date the message gives.
    RelativeDayDisagrees,
    /// A date does not exist, such as 2/30.
    NoSuchDate,
    /// A span ends before it starts.
    BackwardsSpan,
    /// A span starts or ends on a weekend.
    Weekend,
    /// A span covers more than 30 days.
    LongSpan,
    /// The thread's author replied to their own post, which may amend it.
    AuthorReply,
}

impl fmt::Display for Doubt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoKind => f.write_str("no kind"),
            Self::TwoKinds => f.write_str("two kinds"),
            Self::UnreadDateWords(words) => write!(f, "unread date words: {words}"),
            Self::WeekdayDisagrees => f.write_str("weekday disagrees with date"),
            Self::RelativeDayDisagrees => f.write_str("today or tomorrow disagrees with date"),
            Self::NoSuchDate => f.write_str("no such date"),
            Self::BackwardsSpan => f.write_str("span ends before it starts"),
            Self::Weekend => f.write_str("starts or ends on a weekend"),
            Self::LongSpan => f.write_str("span over 30 days"),
            Self::AuthorReply => f.write_str("author replied in own thread"),
        }
    }
}

const MONTHS: &str =
    "jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?";
const WEEKDAYS: &str = "mon|tues?|wed(?:nes)?|thu(?:rs?)?|fri|sat(?:ur)?|sun";

fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("pattern compiles")
}

static CODED_KINDS: LazyLock<[(Kind, Regex); 4]> = LazyLock::new(|| {
    [
        (
            Kind::Remote,
            pattern(r"\bwfh\b|\bwork(?:ing)?\s+(?:from|at)\s+home\b|\b(?:work(?:ing)?\s+)?remote(?:ly)?\b"),
        ),
        (
            Kind::Sick,
            pattern(r"\bsic\b|\bfamsic\b|\bsick\s+(?:days?|time|hours?|leave)\b|\bout\s+sick\b"),
        ),
        (Kind::Vacation, pattern(r"\bvac\b|\bvacation\b")),
        (Kind::Personal, pattern(r"\bpers\b|\bpersonal\b")),
    ]
});
static OFF: LazyLock<Regex> = LazyLock::new(|| pattern(r"\boff\b"));
static OUT: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\boff\b|\bout\s+of\s+(?:the\s+)?office\b|\booo\b|\b(?:be|am|i'm|im)\s+(?:out|gone|away)\b"));
static NOISE: LazyLock<Regex> = LazyLock::new(|| pattern(r"<[^>]*>|\bgood\s+morning\b"));
static HALF_DAY: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b1/2(\s+(?:a\s+)?day)"));
static AMOUNT_NOTE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\b\d+\s*(?:hours?|hrs?|days?|vac|pers|sic)\b|\b\d+\s+(?:vacation|personal|sick)\s+(?:days?|hours?)\b"));
static DAY_PART: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"(?x)
        \b(?P<lead>this|in\s+the|the|today|tomorrow|(?:{WEEKDAYS})(?:day)?|\d+\)?)\s+(?P<part>morning|afternoon)\b
        | \b(?:(?P<first>first)\s+|(?P<second>second)\s+)?(?P<half>half)(?:[\s-]+(?:a\s+)?day|\s+of\s+the\s+day)?\b"
    ))
});
static WHOLE_DAY: LazyLock<Regex> = LazyLock::new(|| pattern(r"\bsick\s+day\b"));
static WEEK: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\b(?:all\s+)?(?:of\s+)?(?:(?:for\s+)?the\s+rest\s+of\s+)?(?P<which>this|next|the)\s+week\b"));
static ATOM: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"(?x)
        \b(?P<m>\d{{1,2}})/(?P<d>\d{{1,2}})(?:/(?P<y>\d{{4}}|\d{{2}}))?\b
        | \b(?P<month>{MONTHS}|may)\.?\s+(?P<md>\d{{1,2}})(?:st|nd|rd|th)?\b(?:,?\s+(?P<my>\d{{4}})\b)?
        | \b(?P<bare>\d{{1,2}})(?:st|nd|rd|th)?\b
        | \b(?P<rel>today|tomorrow)\b
        | \b(?:(?:this|upcoming)\s+)*(?:(?P<next>next)\s+)?(?P<wd>{WEEKDAYS})(?:day)?\b"
    ))
});
static DATE_WORD: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"\d+|\b(?:{WEEKDAYS})(?:day)?\b|\b(?:{MONTHS})\b|\b(?:weeks?|months?|next|last|until|till|yesterday|mornings?|afternoons?|evening|half|tmrw|tonight|weekend|noon|hours?)\b"
    ))
});

/// Reads one message posted on `posted` into its absences.
///
/// # Errors
///
/// Returns every doubt found when the message does not fit the strict grammar.
pub fn read(text: &str, posted: Date) -> Result<Vec<Absence>, Vec<Doubt>> {
    let text = NOISE.replace_all(&text.to_lowercase().replace('’', "'"), " ").into_owned();
    let mut text = HALF_DAY.replace_all(&text, "half$1").into_owned();
    let whole_text = text.clone();
    let mut doubts = Vec::new();

    let coded: Vec<Kind> = CODED_KINDS.iter().filter(|(_, regex)| regex.is_match(&text)).map(|(kind, _)| *kind).collect();
    let kind = match coded[..] {
        [Kind::Remote] if OFF.is_match(&text) => Err(Doubt::TwoKinds),
        [kind] => Ok(kind),
        [] if OUT.is_match(&text) => Ok(Kind::Out),
        [] => Err(Doubt::NoKind),
        _ => Err(Doubt::TwoKinds),
    };
    for regex in [&*AMOUNT_NOTE, &*OUT].into_iter().chain(CODED_KINDS.iter().map(|(_, regex)| regex)) {
        text = blank(&text, regex.find_iter(&text).map(|found| found.range()));
    }

    let mut claims = Vec::new();
    let mut needs_posting_day = false;
    for found in DAY_PART.captures_iter(&text) {
        let (claim, word) = match (found.name("part"), found.name("first"), found.name("second")) {
            (Some(word), ..) if word.as_str() == "morning" => (Part::Morning, word),
            (Some(word), ..) => (Part::Afternoon, word),
            (None, Some(_), _) => (Part::Morning, found.get(0).expect("whole match")),
            (None, None, Some(_)) => (Part::Afternoon, found.get(0).expect("whole match")),
            (None, None, None) => (Part::Half, found.get(0).expect("whole match")),
        };
        needs_posting_day |= found.name("lead").is_some_and(|lead| lead.as_str().starts_with("in"));
        claims.push((claim, word.range()));
    }
    let mut days = Days::find(&text, posted, &mut doubts);
    text = blank(&text, days.consumed.iter().cloned());

    let weeks: Vec<_> = WEEK.captures_iter(&text).collect();
    if let [week] = &weeks[..]
        && days.consumed.is_empty()
    {
        let since_monday = i64::from(posted.weekday().to_monday_zero_offset());
        let next_monday = posted + (7 - since_monday).days();
        if &week["which"] == "next" {
            days.spans = vec![(next_monday, next_monday + 4.days())];
        } else if since_monday <= 4 {
            days.spans = vec![(posted, posted + (4 - since_monday).days())];
        }
        let range = week.get(0).expect("whole match").range();
        text = blank(&text, [range]);
    }

    if WHOLE_DAY.is_match(&whole_text) && claims.iter().all(|(claim, _)| *claim != Part::Half) {
        text = blank(&text, claims.drain(..).map(|(_, range)| range));
    }
    let mut which_half: Vec<Part> = claims.iter().map(|(claim, _)| *claim).filter(|claim| *claim != Part::Half).collect();
    which_half.dedup();
    let mut part = Part::Full;
    if let [(first, last)] = days.spans[..]
        && first == last
        && (first == posted || !needs_posting_day)
        && !claims.is_empty()
        && which_half.len() <= 1
    {
        part = which_half.first().copied().unwrap_or(Part::Half);
        text = blank(&text, claims.into_iter().map(|(_, range)| range));
    }

    let unread: Vec<&str> = DATE_WORD.find_iter(&text).map(|found| found.as_str()).collect();
    if !unread.is_empty() {
        doubts.push(Doubt::UnreadDateWords(unread.join(" ")));
    }
    for &(first, last) in &days.spans {
        if last < first {
            doubts.push(Doubt::BackwardsSpan);
        } else if (last - first).get_days() > 30 {
            doubts.push(Doubt::LongSpan);
        }
        if first == last && matches!(first.weekday(), Weekday::Saturday | Weekday::Sunday) {
            doubts.push(Doubt::Weekend);
        }
    }

    match kind {
        Ok(kind) if doubts.is_empty() => Ok(days.spans.iter().map(|&(first, last)| Absence { kind, first, last, part }).collect()),
        Ok(_) => Err(doubts),
        Err(doubt) => {
            doubts.insert(0, doubt);
            Err(doubts)
        }
    }
}

fn blank(text: &str, ranges: impl IntoIterator<Item = std::ops::Range<usize>>) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for range in ranges {
        bytes[range].fill(b' ');
    }
    String::from_utf8(bytes).expect("whole matches are blanked, so the text stays UTF-8")
}

/// A day word that has no month and day of its own.
enum Loose {
    Relative(Date),
    Named(Weekday),
}

struct Days {
    spans: Vec<(Date, Date)>,
    consumed: Vec<std::ops::Range<usize>>,
}

impl Days {
    fn find(text: &str, posted: Date, doubts: &mut Vec<Doubt>) -> Self {
        let mut dated: Vec<(Date, Date)> = Vec::new();
        let mut loose: Vec<(Loose, bool)> = Vec::new();
        let mut consumed: Vec<std::ops::Range<usize>> = Vec::new();
        let mut since_dated = String::new();
        let mut month_is_named = false;
        let mut previous_is_loose = false;
        let mut names_next_weekday = false;

        for atom in ATOM.captures_iter(text) {
            let whole = atom.get(0).expect("whole match");
            let number = |name: &str| atom.name(name).map(|digits| digits.as_str().parse::<i16>().expect("digits"));
            let since_previous = &text[consumed.last().map_or(0, |range| range.end)..whole.start()];
            since_dated.push_str(since_previous);
            let joins_span = is_span_word(&since_dated);

            if let Some(day) = number("bare") {
                let lists_another = month_is_named && matches!(since_dated.trim_matches([' ', ',']), "and" | "&" | "" | "and the");
                let Some(span) = dated.last().copied().filter(|_| joins_span || lists_another) else {
                    continue;
                };
                match Date::new(span.1.year(), span.1.month(), i8::try_from(day).unwrap_or(0)) {
                    Ok(day) if joins_span => dated.last_mut().expect("span start").1 = day,
                    Ok(day) => dated.push((day, day)),
                    Err(_) => doubts.push(Doubt::NoSuchDate),
                }
            } else if let Some(word) = atom.name("rel") {
                let day = if word.as_str() == "today" { posted } else { posted + 1.day() };
                loose.push((Loose::Relative(day), previous_is_loose && is_span_word(since_previous)));
            } else if let Some(word) = atom.name("wd") {
                loose.push((
                    Loose::Named(weekday_named(word.as_str())),
                    previous_is_loose && is_span_word(since_previous),
                ));
            } else {
                let (month, day, year) = match atom.name("month") {
                    Some(name) => (month_named(name.as_str()), number("md"), number("my")),
                    None => (number("m").expect("numeric date"), number("d"), number("y")),
                };
                month_is_named = atom.name("month").is_some();
                let year = year.map(|year| if year < 100 { year + 2000 } else { year });
                let span_start = dated.last().filter(|_| joins_span).map(|span| span.0);
                let not_before = span_start.unwrap_or(posted - 30.days());
                match (occurrence(month, day.expect("day"), year, not_before), span_start) {
                    (Some(last), Some(_)) => dated.last_mut().expect("span start").1 = last,
                    (Some(day), None) => dated.push((day, day)),
                    (None, _) => doubts.push(Doubt::NoSuchDate),
                }
            }
            names_next_weekday |= atom.name("next").is_some();
            previous_is_loose = atom.name("rel").is_some() || atom.name("wd").is_some();
            if !previous_is_loose {
                since_dated.clear();
            }
            consumed.push(whole.range());
        }

        if dated.is_empty() {
            if names_next_weekday {
                doubts.push(Doubt::UnreadDateWords("next".into()));
            }
            if doubts.is_empty() {
                dated = loose_days(loose, posted, doubts);
            }
            return Self { spans: dated, consumed };
        }
        let ends_on = |fits: &dyn Fn(Date) -> bool| dated.iter().any(|&(first, last)| fits(first) || fits(last));
        for (word, _) in &loose {
            match word {
                Loose::Relative(day) if !ends_on(&|end| end == *day) => doubts.push(Doubt::RelativeDayDisagrees),
                Loose::Named(weekday) if !ends_on(&|end| end.weekday() == *weekday) => doubts.push(Doubt::WeekdayDisagrees),
                _ => {}
            }
        }
        Self { spans: dated, consumed }
    }
}

fn is_span_word(between: &str) -> bool {
    matches!(between.trim_matches([' ', '(', ')', ',']), "-" | "–" | "to" | "through" | "thru")
}

/// Turns `today`, `tomorrow` and weekday names into days when the message gives no month and day.
///
/// A weekday name is its next occurrence after the posting day. A message with no day word is the posting day.
fn loose_days(loose: Vec<(Loose, bool)>, posted: Date, doubts: &mut Vec<Doubt>) -> Vec<(Date, Date)> {
    let mut spans: Vec<(Date, Date)> = Vec::new();
    for (word, joins_span) in loose {
        let previous = spans.last().copied();
        let day = match word {
            Loose::Relative(day) => day,
            Loose::Named(weekday) if !joins_span && previous.is_some_and(|(_, last)| last.weekday() == weekday) => continue,
            Loose::Named(weekday) if !joins_span && weekday == posted.weekday() => {
                doubts.push(Doubt::UnreadDateWords("weekday of the posting day".into()));
                continue;
            }
            Loose::Named(weekday) => {
                let after = previous.filter(|_| joins_span).map_or(posted, |(first, _)| first);
                (1..=7)
                    .map(|ahead| after + ahead.days())
                    .find(|day| day.weekday() == weekday)
                    .expect("a weekday recurs within 7 days")
            }
        };
        match spans.last_mut() {
            Some(span) if joins_span => span.1 = day,
            Some(span) if *span == (day, day) => {}
            _ => spans.push((day, day)),
        }
    }
    if spans.is_empty() && doubts.is_empty() {
        spans.push((posted, posted));
    }
    spans
}

/// The date with this month and day, in `year` if given, else its first occurrence on or after `not_before`.
fn occurrence(month: i16, day: i16, year: Option<i16>, not_before: Date) -> Option<Date> {
    let on = |year: i16| Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok();
    match year {
        Some(year) => on(year),
        None => [0, 1, 2, 3, 4]
            .iter()
            .filter_map(|ahead| on(not_before.year() + ahead))
            .find(|date| *date >= not_before),
    }
}

fn month_named(name: &str) -> i16 {
    const STARTS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let index = STARTS.iter().position(|start| name.starts_with(start)).expect("month name");
    i16::try_from(index + 1).expect("at most 12")
}

fn weekday_named(name: &str) -> Weekday {
    match &name[..2] {
        "mo" => Weekday::Monday,
        "tu" => Weekday::Tuesday,
        "we" => Weekday::Wednesday,
        "th" => Weekday::Thursday,
        "fr" => Weekday::Friday,
        "sa" => Weekday::Saturday,
        _ => Weekday::Sunday,
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    #[expect(clippy::unnecessary_wraps, reason = "compared with what `read` returns")]
    fn days(kind: Kind, spans: &[(Date, Date)]) -> Result<Vec<Absence>, Vec<Doubt>> {
        Ok(spans
            .iter()
            .map(|&(first, last)| Absence {
                kind,
                first,
                last,
                part: Part::Full,
            })
            .collect())
    }

    fn day(kind: Kind, on: Date) -> Result<Vec<Absence>, Vec<Doubt>> {
        days(kind, &[(on, on)])
    }

    const MON_NOV_23: Date = date(2026, 11, 23);

    #[test]
    fn kind_with_no_date_is_the_posting_day() {
        for text in [
            "I'm WFH",
            "I’m also WFH today",
            "i will be working from home today",
            "Working remote, storm is coming",
            "I'm working at home today as well",
        ] {
            assert_eq!(read(text, MON_NOV_23), day(Kind::Remote, MON_NOV_23), "{text}");
        }
        for text in [
            "Taking sick day (SIC)",
            "Need another sick day",
            "Not feeling well, taking a sic day (SIC)",
            "taking a sick day today [SIC] (fever)",
        ] {
            assert_eq!(read(text, MON_NOV_23), day(Kind::Sick, MON_NOV_23), "{text}");
        }
        assert_eq!(read("Taking today off (PERS)", MON_NOV_23), day(Kind::Personal, MON_NOV_23));
    }

    #[test]
    fn feeling_sick_while_remote_is_remote() {
        let read = read("WFH today and tomorrow for me (still sick)", MON_NOV_23);
        assert_eq!(
            read,
            days(Kind::Remote, &[(MON_NOV_23, MON_NOV_23), (date(2026, 11, 24), date(2026, 11, 24))])
        );
    }

    #[test]
    fn tomorrow_is_the_day_after_posting() {
        assert_eq!(read("I'm WFH tomorrow", date(2026, 11, 18)), day(Kind::Remote, date(2026, 11, 19)));
        assert_eq!(
            read("I may WFH tomorrow, getting home quite late", date(2026, 8, 24)),
            day(Kind::Remote, date(2026, 8, 25))
        );
    }

    #[test]
    fn numeric_dates_lists_and_spans() {
        let posted = date(2026, 11, 4);
        assert_eq!(
            read("I will be taking 11/6, 11/9 and 11/10 off (VAC)", posted),
            days(
                Kind::Vacation,
                &[
                    (date(2026, 11, 6), date(2026, 11, 6)),
                    (date(2026, 11, 9), date(2026, 11, 9)),
                    (date(2026, 11, 10), date(2026, 11, 10))
                ]
            )
        );
        assert_eq!(
            read("I will be WFH from 10/28 - 10/30", date(2026, 10, 26)),
            days(Kind::Remote, &[(date(2026, 10, 28), date(2026, 10, 30))])
        );
        assert_eq!(
            read("I will be WFH from 9/16 - 9/18 + 9/21", date(2026, 9, 14)),
            days(
                Kind::Remote,
                &[(date(2026, 9, 16), date(2026, 9, 18)), (date(2026, 9, 21), date(2026, 9, 21))]
            )
        );
        assert_eq!(
            read(
                "I will be be taking 12/22-12/23 and 12/29-12/30 off. 32 hours VAC",
                date(2025, 12, 16)
            ),
            days(
                Kind::Vacation,
                &[(date(2025, 12, 22), date(2025, 12, 23)), (date(2025, 12, 29), date(2025, 12, 30))]
            )
        );
        assert_eq!(
            read("I will be taking 12/17 to 12/22 (32 hours) off (VAC)", date(2025, 12, 15)),
            days(Kind::Vacation, &[(date(2025, 12, 17), date(2025, 12, 22))])
        );
    }

    #[test]
    fn a_date_with_no_year_is_its_next_occurrence() {
        assert_eq!(
            read("I'll take 1/2 off for vacation (VAC)", date(2025, 12, 22)),
            day(Kind::Vacation, date(2026, 1, 2))
        );
        assert_eq!(
            read("VAC 12/23 - 1/4", date(2026, 6, 1)),
            days(Kind::Vacation, &[(date(2026, 12, 23), date(2027, 1, 4))])
        );
        assert_eq!(read("I was out 11/20 (SIC)", MON_NOV_23), day(Kind::Sick, date(2026, 11, 20)));
    }

    #[test]
    fn month_names() {
        assert_eq!(
            read("I'll taking November 25 off for vacation (VAC)", MON_NOV_23),
            day(Kind::Vacation, date(2026, 11, 25))
        );
        assert_eq!(
            read(
                "I'll be taking a vacation on November 9-10 and November 12-13 (VAC)",
                date(2026, 10, 9)
            ),
            days(
                Kind::Vacation,
                &[(date(2026, 11, 9), date(2026, 11, 10)), (date(2026, 11, 12), date(2026, 11, 13))]
            )
        );
        assert_eq!(
            read("I'm not feeling well today, taking the day off Jan 9 (SIC)", date(2026, 1, 9)),
            day(Kind::Sick, date(2026, 1, 9))
        );
    }

    #[test]
    fn weekday_and_relative_words_that_agree_with_the_date() {
        assert_eq!(
            read("I'm taking Monday November 16 off for vacation (VAC)", date(2026, 11, 14)),
            day(Kind::Vacation, date(2026, 11, 16))
        );
        assert_eq!(
            read(
                "I'm going to take tomorrow as a vacation day (Friday, October 23rd - VAC)",
                date(2026, 10, 22)
            ),
            day(Kind::Vacation, date(2026, 10, 23))
        );
        assert_eq!(
            read("I will be taking a vacation day on Monday, 9/21 (8 hours VAC)", date(2026, 9, 17)),
            day(Kind::Vacation, date(2026, 9, 21))
        );
        assert_eq!(
            read("I need to work from home tomorrow (Dec 5th, Friday).", date(2025, 12, 4)),
            day(Kind::Remote, date(2025, 12, 5))
        );
        assert_eq!(
            read("I'm taking tomorrow 12/29 and 12/30 off", date(2025, 12, 28)),
            days(
                Kind::Out,
                &[(date(2025, 12, 29), date(2025, 12, 29)), (date(2025, 12, 30), date(2025, 12, 30))]
            )
        );
    }

    #[test]
    fn uncoded_time_off_is_out() {
        assert_eq!(
            read("Taking tomorrow (10/29) off", date(2026, 10, 28)),
            day(Kind::Out, date(2026, 10, 29))
        );
        assert_eq!(
            read("I will be out of the office today and tomorrow for a conference", date(2026, 8, 31)),
            days(
                Kind::Out,
                &[(date(2026, 8, 31), date(2026, 8, 31)), (date(2026, 9, 1), date(2026, 9, 1))]
            )
        );
        assert_eq!(
            read("Calling it now, I need a day off tomorrow (8 hours VAC)", date(2026, 9, 8)),
            day(Kind::Vacation, date(2026, 9, 9))
        );
    }

    #[test]
    fn part_of_the_posting_day() {
        let morning = Absence {
            kind: Kind::Remote,
            first: MON_NOV_23,
            last: MON_NOV_23,
            part: Part::Morning,
        };
        assert_eq!(read("I will be WFH this morning", MON_NOV_23), Ok(vec![morning]));
        assert_eq!(read("WFH in the morning", MON_NOV_23), Ok(vec![morning]));
        assert!(read("I am WFH tomorrow, and will not be available in the morning", MON_NOV_23).is_err());
        assert!(read("taking this afternoon and tomorrow morning off", MON_NOV_23).is_err());
    }

    #[test]
    fn doubts() {
        let doubts = |text, posted| read(text, posted).unwrap_err();
        assert_eq!(doubts("Oh no… Get better soon!", MON_NOV_23), [Doubt::NoKind]);
        assert!(doubts("I will be taking 11/23 - 11/25 off (PERS + 16 VAC)", date(2026, 11, 18)).contains(&Doubt::TwoKinds));
        assert!(
            doubts(
                "I'm working from home today and will take Wednesday 12/10 off (personal time)",
                date(2025, 12, 8)
            )
            .contains(&Doubt::TwoKinds)
        );
        assert_eq!(
            doubts("I will be taking a personal day (PERS) Wednesday 9/14", date(2026, 9, 14)),
            [Doubt::WeekdayDisagrees]
        );
        assert_eq!(
            doubts("I'm going to take tomorrow 12/31 off, VAC", date(2025, 12, 31)),
            [Doubt::RelativeDayDisagrees]
        );
        assert_eq!(
            doubts("WFH Monday", date(2025, 12, 8)),
            [Doubt::UnreadDateWords("weekday of the posting day".into())]
        );
        assert_eq!(
            doubts("I am working from home today and I will also take Friday off.", date(2020, 2, 26)),
            [Doubt::TwoKinds]
        );
        assert_eq!(doubts("WFH tomorrow", date(2026, 11, 20)), [Doubt::Weekend]);
        assert_eq!(doubts("VAC 2/30", date(2026, 2, 2)), [Doubt::NoSuchDate]);
        assert_eq!(doubts("VAC 6/1 - 8/14", date(2026, 5, 1)), [Doubt::LongSpan]);
        assert!(
            doubts("I took the day to rest after my shots yesterday (SIC)", MON_NOV_23)
                .contains(&Doubt::UnreadDateWords("yesterday".into()))
        );
    }

    #[test]
    fn a_weekday_with_no_date_is_its_next_occurrence() {
        assert_eq!(read("I'll be WFH Monday.", date(2025, 12, 5)), day(Kind::Remote, date(2025, 12, 8)));
        assert_eq!(
            read("I will take Monday off for vacation (VAC)", date(2025, 7, 26)),
            day(Kind::Vacation, date(2025, 7, 28))
        );
        assert_eq!(
            read("I will be taking tomorrow and friday off", date(2025, 5, 21)),
            days(
                Kind::Out,
                &[(date(2025, 5, 22), date(2025, 5, 22)), (date(2025, 5, 23), date(2025, 5, 23))]
            )
        );
        assert_eq!(
            read("I will work from home tomorrow (Thursday)", date(2025, 8, 27)),
            day(Kind::Remote, date(2025, 8, 28))
        );
        assert_eq!(
            read("I will be out Wed - Fri", date(2026, 6, 1)),
            days(Kind::Out, &[(date(2026, 6, 3), date(2026, 6, 5))])
        );
    }

    #[test]
    fn older_phrasings() {
        assert_eq!(
            read("I will be out sick today (1/10)", date(2022, 1, 10)),
            day(Kind::Sick, date(2022, 1, 10))
        );
        assert_eq!(
            read("I'll be out 9/30-10/4", date(2019, 9, 20)),
            days(Kind::Out, &[(date(2019, 9, 30), date(2019, 10, 4))])
        );
        assert_eq!(
            read("I will be on vacation 1/11-1/25 (9 vacation days)", date(2020, 1, 8)),
            days(Kind::Vacation, &[(date(2020, 1, 11), date(2020, 1, 25))])
        );
        assert_eq!(
            read("I will take Aug 19 and 20 off.", date(2020, 8, 6)),
            days(
                Kind::Out,
                &[(date(2020, 8, 19), date(2020, 8, 19)), (date(2020, 8, 20), date(2020, 8, 20))]
            )
        );
        let afternoon = Absence {
            kind: Kind::Sick,
            first: date(2023, 3, 23),
            last: date(2023, 3, 23),
            part: Part::Afternoon,
        };
        assert_eq!(
            read("I'm taking the afternoon off tomorrow (3/23), sick time", date(2023, 3, 22)),
            Ok(vec![afternoon])
        );
    }

    #[test]
    fn weeks_and_half_days() {
        assert_eq!(
            read("I will be WFH all this week", date(2025, 12, 1)),
            days(Kind::Remote, &[(date(2025, 12, 1), date(2025, 12, 5))])
        );
        assert_eq!(
            read("I'm taking the rest of the week off", date(2023, 9, 27)),
            days(Kind::Out, &[(date(2023, 9, 27), date(2023, 9, 29))])
        );
        assert_eq!(
            read("i will be remote next week", date(2026, 8, 13)),
            days(Kind::Remote, &[(date(2026, 8, 17), date(2026, 8, 21))])
        );
        assert_eq!(
            read("I will be taking next Tuesday, June 30th off.", date(2020, 6, 25)),
            day(Kind::Out, date(2020, 6, 30))
        );
        assert!(read("I will be out next Friday", date(2019, 8, 19)).is_err());
        assert_eq!(
            read(
                "I will take Wednesday (Dec 17th) to next Tuesday (Dec 23th) off. 40 hours VAC.",
                date(2025, 12, 15)
            ),
            days(Kind::Vacation, &[(date(2025, 12, 17), date(2025, 12, 23))])
        );
        let half = |kind, on, part| {
            Ok(vec![Absence {
                kind,
                first: on,
                last: on,
                part,
            }])
        };
        assert_eq!(
            read("I will be taking a 1/2 day off today for a dentist appointment.", MON_NOV_23),
            half(Kind::Out, MON_NOV_23, Part::Half)
        );
        assert_eq!(
            read("I'm taking a half personal day tomorrow morning (1/8)", date(2021, 1, 7)),
            half(Kind::Personal, date(2021, 1, 8), Part::Morning)
        );
        assert_eq!(
            read("will be taking off the first half today", MON_NOV_23),
            half(Kind::Out, MON_NOV_23, Part::Morning)
        );
        assert_eq!(
            read("I am taking Wednesday (02/22) afternoon off SIC.", date(2023, 2, 20)),
            half(Kind::Sick, date(2023, 2, 22), Part::Afternoon)
        );
        assert_eq!(
            read("Good morning, I'm taking another sick day today (3/7)", date(2023, 3, 7)),
            day(Kind::Sick, date(2023, 3, 7))
        );
        assert!(read("I'm at an appointment this morning and working from home in the afternoon", MON_NOV_23).is_err());
    }

    #[test]
    fn a_whole_day_with_a_morning_remark_is_a_whole_day() {
        assert_eq!(
            read("Not feeling well this morning, taking a sick day (SIC)", MON_NOV_23),
            day(Kind::Sick, MON_NOV_23)
        );
        let morning = Absence {
            kind: Kind::Personal,
            first: date(2022, 10, 4),
            last: date(2022, 10, 4),
            part: Part::Morning,
        };
        assert_eq!(
            read(
                "I will take next Tuesday morning off (10/04, 3 hours personal day)",
                date(2022, 10, 2)
            ),
            Ok(vec![morning])
        );
        assert_eq!(
            read(
                "I tested positive, I will take Monday and Tuesday off as sick days.",
                date(2023, 8, 27)
            ),
            days(
                Kind::Sick,
                &[(date(2023, 8, 28), date(2023, 8, 28)), (date(2023, 8, 29), date(2023, 8, 29))]
            )
        );
    }

    #[test]
    fn slack_mentions_are_not_date_words() {
        assert_eq!(read("<@U0123ABCDEF> I'm WFH too", MON_NOV_23), day(Kind::Remote, MON_NOV_23));
    }
}
