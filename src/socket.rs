//! Slack's Socket Mode: Slack pushes events over a WebSocket and the listener signals a pull.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;

use crate::slack::Slack;

const SILENCE_LIMIT: Duration = Duration::from_secs(30);

/// What one frame from Slack asks of the listener.
#[derive(Debug, Default, PartialEq, Eq)]
struct Reaction {
    /// The envelope to acknowledge, so that Slack does not send it again.
    acknowledge: Option<String>,
    /// The channel may have changed.
    pull: bool,
    /// Slack is about to close this connection.
    reconnect: bool,
}

fn reaction_to(frame: &str, channel: &str) -> Reaction {
    #[derive(Deserialize)]
    struct Frame {
        #[serde(rename = "type")]
        kind: Option<String>,
        envelope_id: Option<String>,
        #[serde(default)]
        payload: Value,
    }
    let Ok(frame) = serde_json::from_str::<Frame>(frame) else {
        return Reaction::default();
    };
    let kind = frame.kind.as_deref();
    let event = &frame.payload["event"];
    Reaction {
        acknowledge: frame.envelope_id,
        // Slack does not promise the events sent while no connection was open, so each `hello` pulls.
        pull: kind == Some("hello") || (kind == Some("events_api") && event["type"] == "message" && event["channel"] == channel),
        reconnect: kind == Some("disconnect"),
    }
}

/// Holds a Socket Mode connection, replacing it whenever Slack closes it, and notifies `changed` on each
/// connect and on each message event of `channel`.
///
/// `app` must hold the app-level token. Only one process may watch at a time, because Slack sends each
/// event to one of the app's open connections.
///
/// # Errors
///
/// Fails when Slack refuses a connection, the socket breaks, or the socket stays silent after a ping.
pub async fn watch(app: &Slack, channel: &str, changed: &Notify) -> Result<()> {
    loop {
        let url = app.socket_url().await?;
        follow(&url, channel, changed, SILENCE_LIMIT).await?;
    }
}

/// Follows one connection until Slack closes it.
async fn follow(url: &str, channel: &str, changed: &Notify, silence_limit: Duration) -> Result<()> {
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.context("cannot connect to the socket")?;
    let (mut events, mut pulls) = (0, 0);
    let mut pinged = false;
    loop {
        let frame = match tokio::time::timeout(silence_limit, socket.next()).await {
            Ok(Some(frame)) => frame.context("socket broke")?,
            Ok(None) => break,
            Err(_) if pinged => bail!("socket silent for {silence_limit:?} after a ping"),
            Err(_) => {
                socket.send(Message::Ping(Vec::new().into())).await.context("cannot ping")?;
                pinged = true;
                continue;
            }
        };
        pinged = false;
        let Message::Text(text) = frame else {
            continue;
        };
        let reaction = reaction_to(&text, channel);
        if let Some(envelope_id) = reaction.acknowledge {
            let acknowledgment = json!({ "envelope_id": envelope_id }).to_string();
            socket.send(Message::Text(acknowledgment.into())).await.context("cannot acknowledge")?;
            events += 1;
        }
        if reaction.pull {
            changed.notify_one();
            pulls += 1;
        }
        if reaction.reconnect {
            break;
        }
    }
    eprintln!(
        "{} connection closed: {events} events acknowledged, {pulls} pulls signaled",
        jiff::Timestamp::now()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;
    use tokio_tungstenite::WebSocketStream;

    use super::*;

    const CHANNEL: &str = "C0STATUS";

    fn event(envelope_id: &str, channel: &str, subtype: Option<&str>) -> String {
        json!({
            "type": "events_api",
            "envelope_id": envelope_id,
            "payload": { "event": { "type": "message", "subtype": subtype, "channel": channel, "ts": "1.2" } },
        })
        .to_string()
    }

    #[test]
    fn reactions() {
        let acknowledged = |id: &str, pull| Reaction {
            acknowledge: Some(id.into()),
            pull,
            reconnect: false,
        };
        assert_eq!(
            reaction_to(r#"{"type":"hello","num_connections":1}"#, CHANNEL),
            Reaction {
                pull: true,
                ..Reaction::default()
            }
        );
        assert_eq!(reaction_to(&event("a", CHANNEL, None), CHANNEL), acknowledged("a", true));
        assert_eq!(
            reaction_to(&event("b", CHANNEL, Some("message_changed")), CHANNEL),
            acknowledged("b", true)
        );
        assert_eq!(
            reaction_to(&event("c", CHANNEL, Some("message_deleted")), CHANNEL),
            acknowledged("c", true)
        );
        assert_eq!(reaction_to(&event("d", "C0OTHER", None), CHANNEL), acknowledged("d", false));
        assert_eq!(
            reaction_to(
                r#"{"type":"events_api","envelope_id":"e","payload":{"event":{"type":"reaction_added"}}}"#,
                CHANNEL
            ),
            acknowledged("e", false)
        );
        assert_eq!(
            reaction_to(r#"{"type":"disconnect","reason":"refresh_requested"}"#, CHANNEL),
            Reaction {
                reconnect: true,
                ..Reaction::default()
            }
        );
        assert_eq!(reaction_to("not json", CHANNEL), Reaction::default());
    }

    /// Serves one WebSocket connection on a free local port with `serve` and returns its URL.
    async fn server<F: Future<Output = ()> + Send + 'static>(
        serve: impl FnOnce(WebSocketStream<tokio::net::TcpStream>) -> F + Send + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(tokio_tungstenite::accept_async(stream).await.unwrap()).await;
        });
        url
    }

    #[tokio::test]
    async fn acknowledges_events_signals_a_pull_and_returns_on_disconnect() {
        let (acknowledged, mut acknowledgments) = tokio::sync::mpsc::unbounded_channel();
        let url = server(async move |mut socket| {
            for frame in [event("first", "C0OTHER", None), event("second", CHANNEL, None)] {
                socket.send(Message::Text(frame.into())).await.unwrap();
                let answer = socket.next().await.unwrap().unwrap();
                acknowledged.send(answer.into_text().unwrap().to_string()).unwrap();
            }
            socket
                .send(Message::Text(r#"{"type":"disconnect","reason":"refresh_requested"}"#.into()))
                .await
                .unwrap();
        })
        .await;
        let changed = Notify::new();

        follow(&url, CHANNEL, &changed, Duration::from_secs(5)).await.unwrap();

        assert_eq!(acknowledgments.recv().await.unwrap(), r#"{"envelope_id":"first"}"#);
        assert_eq!(acknowledgments.recv().await.unwrap(), r#"{"envelope_id":"second"}"#);
        tokio::time::timeout(Duration::from_secs(1), changed.notified()).await.expect("a pull was signaled");
    }

    #[tokio::test]
    async fn an_event_outside_the_channel_signals_no_pull() {
        let url = server(async |mut socket| {
            socket.send(Message::Text(event("only", "C0OTHER", None).into())).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket.close(None).await.unwrap();
        })
        .await;
        let changed = Notify::new();

        follow(&url, CHANNEL, &changed, Duration::from_secs(5)).await.unwrap();

        assert!(tokio::time::timeout(Duration::from_millis(50), changed.notified()).await.is_err());
    }

    #[tokio::test]
    async fn pings_a_silent_socket_then_fails() {
        let (pinged, was_pinged) = tokio::sync::oneshot::channel();
        let url = server(async move |mut socket| {
            pinged.send(socket.next().await.unwrap().unwrap().is_ping()).unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await;

        let error = follow(&url, CHANNEL, &Notify::new(), Duration::from_millis(50)).await.unwrap_err();

        assert!(was_pinged.await.unwrap());
        assert!(error.to_string().contains("silent"), "{error}");
    }
}
