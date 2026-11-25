//! A calendar of who is out, read from a Slack status channel.

mod board;
mod reading;
mod server;
mod slack;
mod socket;
mod store;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use jiff::civil::Date;
use tokio::sync::Notify;

use crate::board::Board;
use crate::slack::{Channel, Slack};
use crate::store::Store;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Directory that holds `whosout.db` and `resolutions.toml` [default: `$XDG_DATA_HOME/whosout`]
    #[arg(long, env = "WHOSOUT_DIR")]
    dir: Option<PathBuf>,
    #[command(flatten)]
    channel: Channel,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pull the whole channel from Slack with the token in `SLACK_USER_TOKEN`
    Sync,
    /// Pull on every change to the channel, pushed by Slack over the socket of `SLACK_APP_TOKEN`
    Listen,
    /// Serve the calendar page
    Serve {
        /// Address to listen on
        #[arg(long, env = "WHOSOUT_LISTEN", default_value = "127.0.0.1:8377")]
        listen: SocketAddr,
    },
    /// Print the messages that need review as JSON lines, newest first
    Pending {
        /// Only messages posted on or after this date
        #[arg(long)]
        since: Option<Date>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let channel = cli.channel;
    let dir = match cli.dir {
        Some(dir) => dir,
        None => match std::env::var_os("XDG_DATA_HOME") {
            Some(data) => PathBuf::from(data).join("whosout"),
            None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/share/whosout"),
        },
    };
    match cli.command {
        Command::Sync => println!(
            "{}",
            sync(&Slack::new(token("SLACK_USER_TOKEN")?, &channel.id), &dir, &channel).await?
        ),
        Command::Listen => {
            let reader = Slack::new(token("SLACK_USER_TOKEN")?, &channel.id);
            let app = Slack::new(token("SLACK_APP_TOKEN")?, &channel.id);
            let changed = Notify::new();
            tokio::try_join!(
                socket::watch(&app, &channel.id, &changed),
                pull_when(&changed, &reader, &dir, &channel)
            )?;
        }
        Command::Serve { listen } => server::serve(listen, dir, channel).await?,
        Command::Pending { since } => {
            let mut store = Store::open(&dir)?;
            let board = Board::load(&store, &dir, &channel)?;
            store.replace_days(&board.days())?;
            let since = since.map_or_default(|since| since.to_string());
            for pending in board.pending().iter().filter(|pending| pending.shown.posted[4..14] >= *since) {
                println!("{}", serde_json::to_string(pending)?);
            }
        }
    }
    Ok(())
}

fn token(variable: &str) -> Result<String> {
    std::env::var(variable).with_context(|| format!("{variable} is not set"))
}

/// Pulls the channel, rebuilds the day table and returns a one-line summary.
async fn sync(slack: &Slack, dir: &Path, channel: &Channel) -> Result<String> {
    let mut store = Store::open(dir)?;
    let (messages, threads) = pull(slack, &mut store).await?;
    let board = Board::load(&store, dir, channel)?;
    store.replace_days(&board.days())?;
    Ok(format!(
        "{messages} messages, {threads} threads fetched, {} need review",
        board.pending().len()
    ))
}

/// Syncs each time `changed` is notified. A failed sync is logged and tried again a minute later.
async fn pull_when(changed: &Notify, slack: &Slack, dir: &Path, channel: &Channel) -> Result<()> {
    loop {
        changed.notified().await;
        // A post and its quick edit arrive as two events. Waiting folds them into one pull.
        tokio::time::sleep(Duration::from_secs(2)).await;
        match sync(slack, dir, channel).await {
            Ok(summary) => eprintln!("{} {summary}", jiff::Timestamp::now()),
            Err(error) => {
                eprintln!("{} sync failed: {error:#}", jiff::Timestamp::now());
                tokio::time::sleep(Duration::from_secs(60)).await;
                changed.notify_one();
            }
        }
    }
}

/// Replaces the cache with the channel's whole history and returns the counts of messages and of threads fetched.
///
/// A thread is fetched only when its newest reply differs from the cached one.
async fn pull(slack: &Slack, store: &mut Store) -> Result<(usize, usize)> {
    let cached = store.messages()?;
    let cached_heads: HashMap<&str, &Option<String>> = cached.iter().map(|message| (message.ts.as_str(), &message.latest_reply)).collect();
    let mut messages = slack.history().await?;
    let mut threads = 0;
    for head in messages
        .clone()
        .iter()
        .filter(|message| message.thread_ts.is_none() && message.latest_reply.is_some())
    {
        if cached_heads.get(head.ts.as_str()) == Some(&&head.latest_reply) {
            messages.extend(cached.iter().filter(|message| message.thread_ts.as_ref() == Some(&head.ts)).cloned());
        } else {
            messages.extend(slack.replies(&head.ts).await?);
            threads += 1;
        }
    }
    messages.sort_by(|a, b| a.ts.cmp(&b.ts));
    messages.dedup_by(|a, b| a.ts == b.ts);
    let users = slack.users().await?;
    store.replace_cache(&messages, &users)?;
    Ok((messages.len(), threads))
}
