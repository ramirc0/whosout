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
        (Kind::Sick, pattern(r"\bsic\b|\bsick\s+day\b")),
        (Kind::Vacation, pattern(r"\bvac\b|\bvacation\b")),
        (Kind::Personal, pattern(r"\bpers\b|\bpersonal\b")),
    ]
});
static OUT: LazyLock<Regex> = LazyLock::new(|| pattern(r"\boff\b|\bout\s+of\s+(?:the\s+)?office\b|\booo\b"));
static SLACK_ENTITY: LazyLock<Regex> = LazyLock::new(|| pattern(r"<[^>]*>"));
static HOURS_NOTE: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b\d+\s*(?:hours?|hrs?)\b"));
static DAY_PART: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b(?:this|in\s+the)\s+(morning|afternoon)\b"));
static ATOM: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"(?x)
        \b(?P<m>\d{{1,2}})/(?P<d>\d{{1,2}})(?:/(?P<y>\d{{4}}|\d{{2}}))?\b
        | \b(?P<month>{MONTHS}|may)\.?\s+(?P<md>\d{{1,2}})(?:st|nd|rd|th)?\b(?:,?\s+(?P<my>\d{{4}})\b)?
        | \b(?P<bare>\d{{1,2}})(?:st|nd|rd|th)?\b
        | \b(?P<rel>today|tomorrow)\b
        | \b(?P<wd>{WEEKDAYS})(?:day)?\b"
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
    let mut text = SLACK_ENTITY.replace_all(&text.to_lowercase().replace('’', "'"), " ").into_owned();
    let mut doubts = Vec::new();

    let coded: Vec<Kind> = CODED_KINDS
        .iter()
        .filter(|(_, regex)| regex.is_match(&text))
        .map(|(kind, _)| *kind)
        .collect();
    let kind = match coded[..] {
        [kind] => Some(kind),
        [] if OUT.is_match(&text) => Some(Kind::Out),
        [] => {
            doubts.push(Doubt::NoKind);
            None
        }
        _ => {
            doubts.push(Doubt::TwoKinds);
            None
        }
    };
    for regex in CODED_KINDS.iter().map(|(_, regex)| regex).chain([&*OUT, &*HOURS_NOTE]) {
        text = blank(&text, regex.find_iter(&text).map(|found| found.range()));
    }

    let days = Days::find(&text, posted, &mut doubts);
    text = blank(&text, days.consumed.iter().cloned());

    let mut part = Part::Full;
    let parts: Vec<_> = DAY_PART.captures_iter(&text).collect();
    if let [only] = &parts[..]
        && days.spans == [(posted, posted)]
    {
        part = if &only[1] == "morning" { Part::Morning } else { Part::Afternoon };
        text = blank(&text, [only.get(0).expect("whole match").range()]);
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
        if [first, last]
            .iter()
            .any(|day| matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday))
        {
            doubts.push(Doubt::Weekend);
        }
    }

    match kind {
        Some(kind) if doubts.is_empty() => Ok(days
            .spans
            .iter()
            .map(|&(first, last)| Absence { kind, first, last, part })
            .collect()),
        _ => Err(doubts),
    }
}

fn blank(text: &str, ranges: impl IntoIterator<Item = std::ops::Range<usize>>) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for range in ranges {
        bytes[range].fill(b' ');
    }
    String::from_utf8(bytes).expect("whole matches are blanked, so the text stays UTF-8")
}

struct Days {
    spans: Vec<(Date, Date)>,
    consumed: Vec<std::ops::Range<usize>>,
}

impl Days {
    fn find(text: &str, posted: Date, doubts: &mut Vec<Doubt>) -> Self {
        let mut explicit: Vec<(Date, Date)> = Vec::new();
        let mut relative: Vec<Date> = Vec::new();
        let mut weekdays: Vec<Weekday> = Vec::new();
        let mut consumed = Vec::new();
        let mut gap = String::new();
        let mut cursor = 0;

        for atom in ATOM.captures_iter(text) {
            let whole = atom.get(0).expect("whole match");
            let number = |name: &str| atom.name(name).map(|digits| digits.as_str().parse::<i16>().expect("digits"));
            gap.push_str(&text[cursor..whole.start()]);
            let joins_span = matches!(gap.trim_matches([' ', '(', ')', ',']), "-" | "–" | "to" | "through" | "thru");

            if let Some(day) = number("bare") {
                let Some(span) = explicit.last_mut().filter(|_| joins_span) else {
                    continue;
                };
                match Date::new(span.0.year(), span.0.month(), i8::try_from(day).unwrap_or(0)) {
                    Ok(last) => span.1 = last,
                    Err(_) => doubts.push(Doubt::NoSuchDate),
                }
            } else if let Some(word) = atom.name("rel") {
                relative.push(if word.as_str() == "today" { posted } else { posted + 1.day() });
            } else if let Some(word) = atom.name("wd") {
                weekdays.push(weekday_named(word.as_str()));
            } else {
                let (month, day, year) = match atom.name("month") {
                    Some(name) => (month_named(name.as_str()), number("md"), number("my")),
                    None => (number("m").expect("numeric date"), number("d"), number("y")),
                };
                let year = year.map(|year| if year < 100 { year + 2000 } else { year });
                let span_start = explicit.last().filter(|_| joins_span).map(|span| span.0);
                let not_before = span_start.unwrap_or(posted - 30.days());
                match (occurrence(month, day.expect("day"), year, not_before), span_start) {
                    (Some(last), Some(_)) => explicit.last_mut().expect("span start").1 = last,
                    (Some(day), None) => explicit.push((day, day)),
                    (None, _) => doubts.push(Doubt::NoSuchDate),
                }
            }
            if atom.name("rel").is_none() && atom.name("wd").is_none() {
                gap.clear();
            }
            cursor = whole.end();
            consumed.push(whole.range());
        }

        let named = |day: &Date| explicit.iter().any(|(first, last)| day == first || day == last);
        if explicit.is_empty() {
            if relative.is_empty() && !weekdays.is_empty() {
                doubts.push(Doubt::UnreadDateWords("weekday with no date".into()));
            } else if weekdays.iter().any(|weekday| !relative.iter().any(|day| day.weekday() == *weekday)) {
                doubts.push(Doubt::WeekdayDisagrees);
            }
            if relative.is_empty() {
                relative.push(posted);
            }
            explicit = relative.into_iter().map(|day| (day, day)).collect();
        } else {
            if !relative.iter().all(named) {
                doubts.push(Doubt::RelativeDayDisagrees);
            }
            let falls_on = |weekday: &Weekday| {
                explicit
                    .iter()
                    .any(|(first, last)| first.weekday() == *weekday || last.weekday() == *weekday)
            };
            if !weekdays.iter().all(falls_on) {
                doubts.push(Doubt::WeekdayDisagrees);
            }
        }
        Self { spans: explicit, consumed }
    }
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
        assert!(read("I will be taking the first half of the day off today", MON_NOV_23).is_err());
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
            doubts("I will be WFH all this week", date(2025, 12, 1)),
            [Doubt::UnreadDateWords("week".into())]
        );
        assert_eq!(
            doubts("I'll be WFH Monday.", date(2025, 12, 5)),
            [Doubt::UnreadDateWords("weekday with no date".into())]
        );
        assert_eq!(doubts("WFH tomorrow", date(2026, 11, 20)), [Doubt::Weekend]);
        assert_eq!(doubts("VAC 2/30", date(2026, 2, 2)), [Doubt::NoSuchDate]);
        assert_eq!(doubts("VAC 6/1 - 8/14", date(2026, 5, 1)), [Doubt::LongSpan]);
        assert!(
            doubts("I took the day to rest after my shots yesterday (SIC)", MON_NOV_23)
                .contains(&Doubt::UnreadDateWords("yesterday".into()))
        );
        assert!(
            doubts(
                "I will take Wednesday (Dec 17th) to next Tuesday (Dec 23th) off. 40 hours VAC.",
                date(2025, 12, 15)
            )
            .iter()
            .any(|doubt| matches!(doubt, Doubt::UnreadDateWords(_)))
        );
    }

    #[test]
    fn slack_mentions_are_not_date_words() {
        assert_eq!(read("<@U0123ABCDEF> I'm WFH too", MON_NOV_23), day(Kind::Remote, MON_NOV_23));
    }
}
