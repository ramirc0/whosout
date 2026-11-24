//! A calendar of who is out, read from the lab's Slack status channel.

mod board;
mod reading;
mod server;
mod slack;
mod store;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use jiff::civil::Date;

use crate::board::Board;
use crate::slack::Slack;
use crate::store::Store;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Directory that holds `whosout.db` and `resolutions.toml` [default: `$XDG_DATA_HOME/whosout`]
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pull the whole channel from Slack with the token in `SLACK_USER_TOKEN`
    Sync,
    /// Serve the calendar page
    Serve {
        /// Address to listen on
        #[arg(long, default_value = "127.0.0.1:8377")]
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
    let dir = match cli.dir {
        Some(dir) => dir,
        None => match std::env::var_os("XDG_DATA_HOME") {
            Some(data) => PathBuf::from(data).join("whosout"),
            None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/share/whosout"),
        },
    };
    match cli.command {
        Command::Sync => {
            let token = std::env::var("SLACK_USER_TOKEN").context("SLACK_USER_TOKEN is not set")?;
            let mut store = Store::open(&dir)?;
            let (messages, threads) = pull(&Slack::new(token), &mut store).await?;
            let board = Board::load(&store, &dir)?;
            store.replace_days(&board.days())?;
            println!(
                "{messages} messages, {threads} threads fetched, {} need review",
                board.pending().len()
            );
        }
        Command::Serve { listen } => server::serve(listen, dir).await?,
        Command::Pending { since } => {
            let mut store = Store::open(&dir)?;
            let board = Board::load(&store, &dir)?;
            store.replace_days(&board.days())?;
            let since = since.map_or_default(|since| since.to_string());
            for pending in board.pending().iter().filter(|pending| pending.shown.posted[4..14] >= *since) {
                println!("{}", serde_json::to_string(pending)?);
            }
        }
    }
    Ok(())
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
            messages.extend(
                cached
                    .iter()
                    .filter(|message| message.thread_ts.as_ref() == Some(&head.ts))
                    .cloned(),
            );
        } else {
            messages.extend(slack.replies(&head.ts).await?);
            threads += 1;
        }
    }
    let users = slack.users().await?;
    store.replace_cache(&messages, &users)?;
    Ok((messages.len(), threads))
}
