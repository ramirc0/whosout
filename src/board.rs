//! The cached channel combined with the hand-written resolutions.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result, bail};
use jiff::ToSpan;
use jiff::civil::{Date, Weekday};
use serde::{Deserialize, Serialize};

use crate::reading::{self, Absence, Doubt, Kind, Part};
use crate::slack::Message;
use crate::store::Store;

/// One person away for one weekday.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    /// The day away.
    pub date: Date,
    /// The person's Slack user id.
    pub user: String,
    /// The person's name.
    pub person: String,
    /// The class of the absence.
    pub kind: Kind,
    /// The part of the day away.
    pub part: Part,
    /// The timestamp of the message that reported the absence.
    pub message_ts: String,
}

/// A message as shown to a reviewer.
#[derive(Debug, Serialize)]
pub struct Shown {
    /// The message's Slack timestamp, which keys its resolution.
    pub message: String,
    /// The author's name.
    pub person: String,
    /// The weekday, date and time of posting in the lab's time zone.
    pub posted: String,
    /// The message text.
    pub text: String,
    /// The message's permalink.
    pub link: String,
}

/// A message that the regex reading refused and no resolution covers.
#[derive(Debug, Serialize)]
pub struct Pending {
    /// The refused message.
    #[serde(flatten)]
    pub shown: Shown,
    /// Why the reading refused it.
    pub doubts: Vec<String>,
    /// The message this one replies to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Shown>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolutions {
    #[serde(default)]
    resolution: Vec<Resolution>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolution {
    message: String,
    absences: Vec<Absence>,
    #[serde(rename = "why")]
    _why: Option<String>,
}

/// Every cached message with the names and resolutions needed to read it.
pub struct Board {
    messages: Vec<Message>,
    names: HashMap<String, String>,
    resolutions: HashMap<String, Vec<Absence>>,
    /// When the last sync finished, if one ever did.
    pub synced: Option<String>,
}

impl Board {
    /// Loads the cache from `store` and the resolutions from `resolutions.toml` in `dir`.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be read, or when `resolutions.toml` is malformed, resolves a message
    /// twice, names a message that is not cached, or holds a span that ends before it starts.
    pub fn load(store: &Store, dir: &Path) -> Result<Self> {
        let messages = store.messages()?;
        let path = dir.join("resolutions.toml");
        let written = match std::fs::read_to_string(&path) {
            Ok(written) => written,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
        };
        let parsed: Resolutions = toml::from_str(&written).with_context(|| format!("malformed {}", path.display()))?;
        let mut resolutions = HashMap::new();
        for resolution in parsed.resolution {
            let ts = resolution.message;
            if !messages.iter().any(|message| message.ts == ts) {
                bail!("{}: no cached message {ts}", path.display());
            }
            if resolution.absences.iter().any(|absence| absence.last < absence.first) {
                bail!("{}: message {ts} has a span that ends before it starts", path.display());
            }
            if resolutions.insert(ts.clone(), resolution.absences).is_some() {
                bail!("{}: message {ts} is resolved twice", path.display());
            }
        }
        Ok(Self {
            messages,
            names: store.names()?,
            resolutions,
            synced: store.synced()?,
        })
    }

    /// The number of cached messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Returns one row per person per weekday away, ordered by person and day.
    ///
    /// When two messages from one person cover the same day, the later post wins.
    pub fn days(&self) -> Vec<Day> {
        let mut days = BTreeMap::new();
        for message in &self.messages {
            for absence in self.verdict(message).unwrap_or_default() {
                let mut day = absence.first;
                while day <= absence.last {
                    if !matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday) {
                        days.insert((self.name(&message.user), message.user.clone(), day), (absence, &message.ts));
                    }
                    day += 1.day();
                }
            }
        }
        days.into_iter()
            .map(|((person, user, date), (absence, ts))| Day {
                date,
                user,
                person,
                kind: absence.kind,
                part: absence.part,
                message_ts: ts.clone(),
            })
            .collect()
    }

    /// Returns the messages that still need review, newest first.
    pub fn pending(&self) -> Vec<Pending> {
        let pending = self.messages.iter().rev().filter_map(|message| {
            let doubts = self.verdict(message).err()?;
            Some(Pending {
                shown: self.shown(message),
                doubts: doubts.iter().map(Doubt::to_string).collect(),
                parent: self.parent(message).map(|parent| self.shown(parent)),
            })
        });
        pending.collect()
    }

    /// Shows the message with timestamp `ts`, if it is cached.
    pub fn message(&self, ts: &str) -> Option<Shown> {
        self.messages
            .iter()
            .find(|message| message.ts == ts)
            .map(|message| self.shown(message))
    }

    fn verdict(&self, message: &Message) -> Result<Vec<Absence>, Vec<Doubt>> {
        if let Some(absences) = self.resolutions.get(&message.ts) {
            return Ok(absences.clone());
        }
        let Some(parent) = self.parent(message) else {
            return reading::read(&message.text, message.posted().date());
        };
        if parent.user == message.user {
            return Err(vec![Doubt::AuthorReply]);
        }
        match reading::read(&message.text, message.posted().date()) {
            Err(doubts) if doubts.contains(&Doubt::NoKind) => Ok(Vec::new()),
            verdict => verdict,
        }
    }

    fn parent(&self, message: &Message) -> Option<&Message> {
        let parent = message.thread_ts.as_ref()?;
        self.messages.iter().find(|message| message.ts == *parent)
    }

    fn name(&self, user: &str) -> String {
        self.names.get(user).cloned().unwrap_or_else(|| user.to_owned())
    }

    fn shown(&self, message: &Message) -> Shown {
        Shown {
            message: message.ts.clone(),
            person: self.name(&message.user),
            posted: message.posted().strftime("%a %Y-%m-%d %H:%M").to_string(),
            text: message.text.clone(),
            link: message.link(),
        }
    }
}
