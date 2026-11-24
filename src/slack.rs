//! The slice of the Slack Web API that the sync needs.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use jiff::ZonedDateTime;
use serde::Deserialize;
use serde_json::Value;

/// The id of `#status-channel`.
pub const CHANNEL: &str = "C0123456789";
const WORKSPACE: &str = "https://example.slack.com";
const LAB_TIME_ZONE: &str = "America/New_York";

/// One person-written message of the channel, top-level or in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Message {
    /// Slack's timestamp, which is also the message's id within the channel.
    pub ts: String,
    /// The author's user id.
    pub user: String,
    /// The message text.
    pub text: String,
    /// The parent's timestamp when this message is a thread reply.
    pub thread_ts: Option<String>,
    /// The timestamp of the newest reply when this message heads a thread.
    pub latest_reply: Option<String>,
}

impl Message {
    /// The moment the message was posted, in the lab's time zone.
    ///
    /// # Panics
    ///
    /// Panics if `ts` is not a Slack timestamp or the time zone database lacks the lab's zone.
    pub fn posted(&self) -> ZonedDateTime {
        let seconds = self
            .ts
            .split('.')
            .next()
            .and_then(|seconds| seconds.parse().ok())
            .expect("Slack timestamp");
        jiff::Timestamp::from_second(seconds)
            .expect("timestamp in range")
            .in_tz(LAB_TIME_ZONE)
            .expect("lab time zone")
    }

    /// The message's permalink.
    pub fn link(&self) -> String {
        let id = self.ts.replace('.', "");
        match &self.thread_ts {
            Some(parent) => format!("{WORKSPACE}/archives/{CHANNEL}/p{id}?thread_ts={parent}&cid={CHANNEL}"),
            None => format!("{WORKSPACE}/archives/{CHANNEL}/p{id}"),
        }
    }
}

#[derive(Deserialize)]
struct RawMessage {
    ts: String,
    user: Option<String>,
    text: Option<String>,
    subtype: Option<String>,
    thread_ts: Option<String>,
    latest_reply: Option<String>,
}

impl RawMessage {
    /// Keeps what a person wrote. Joins, bot posts and other system lines carry a subtype.
    fn written(self) -> Option<Message> {
        if !matches!(self.subtype.as_deref(), None | Some("thread_broadcast")) {
            return None;
        }
        let text = self.text?.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
        let thread_ts = self.thread_ts.filter(|parent| *parent != self.ts);
        Some(Message {
            user: self.user?,
            text,
            thread_ts,
            latest_reply: self.latest_reply,
            ts: self.ts,
        })
    }
}

/// A Slack Web API client bound to one token.
pub struct Slack {
    http: reqwest::Client,
    token: String,
}

impl Slack {
    /// Creates a client that authenticates with `token`.
    pub fn new(token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            token,
        }
    }

    /// Fetches every top-level message of the channel.
    ///
    /// # Errors
    ///
    /// Fails when the request fails or Slack answers with an error.
    pub async fn history(&self) -> Result<Vec<Message>> {
        let raw = self
            .every("conversations.history", &[("channel", CHANNEL), ("limit", "999")], "messages")
            .await?;
        Ok(raw.into_iter().filter_map(RawMessage::written).collect())
    }

    /// Fetches the replies of the thread headed by the message at `parent`.
    ///
    /// # Errors
    ///
    /// Fails when the request fails or Slack answers with an error.
    pub async fn replies(&self, parent: &str) -> Result<Vec<Message>> {
        let query = [("channel", CHANNEL), ("ts", parent), ("limit", "999")];
        let raw = self.every("conversations.replies", &query, "messages").await?;
        Ok(raw
            .into_iter()
            .filter_map(RawMessage::written)
            .filter(|message| message.thread_ts.is_some())
            .collect())
    }

    /// Fetches every user of the workspace as `(id, name)`.
    ///
    /// # Errors
    ///
    /// Fails when the request fails or Slack answers with an error.
    pub async fn users(&self) -> Result<Vec<(String, String)>> {
        #[derive(Deserialize)]
        struct User {
            id: String,
            name: String,
            real_name: Option<String>,
        }
        let users: Vec<User> = self.every("users.list", &[("limit", "999")], "members").await?;
        Ok(users
            .into_iter()
            .map(|user| (user.id, user.real_name.filter(|name| !name.is_empty()).unwrap_or(user.name)))
            .collect())
    }

    /// Calls `method` page by page and collects the array under `key`.
    async fn every<T: serde::de::DeserializeOwned>(&self, method: &str, query: &[(&str, &str)], key: &str) -> Result<Vec<T>> {
        let mut items = Vec::new();
        let mut cursor = String::new();
        loop {
            let mut page = self.call(method, query, &cursor).await?;
            items.extend(serde_json::from_value::<Vec<T>>(page[key].take()).with_context(|| format!("{method}: unexpected {key}"))?);
            cursor = page["response_metadata"]["next_cursor"].as_str().unwrap_or_default().to_owned();
            if cursor.is_empty() {
                return Ok(items);
            }
        }
    }

    async fn call(&self, method: &str, query: &[(&str, &str)], cursor: &str) -> Result<Value> {
        loop {
            let request = self
                .http
                .get(format!("https://slack.com/api/{method}"))
                .bearer_auth(&self.token)
                .query(query);
            let request = if cursor.is_empty() {
                request
            } else {
                request.query(&[("cursor", cursor)])
            };
            let response = request.send().await.with_context(|| format!("{method}: request failed"))?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok()?.parse().ok())
                    .unwrap_or(30);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            let body: Value = response.json().await.with_context(|| format!("{method}: answer is not JSON"))?;
            if body["ok"] != true {
                bail!("{method}: Slack answered {}", body["error"]);
            }
            return Ok(body);
        }
    }
}
