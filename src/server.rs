//! The HTTP server: the calendar page and the JSON it draws from.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::board::{Board, Pending, Shown};
use crate::reading::{Kind, Part};
use crate::store::Store;

#[derive(Deserialize)]
struct Window {
    first: Date,
    last: Date,
}

#[derive(Serialize)]
struct Block {
    day: Date,
    kind: Kind,
    part: Part,
    #[serde(flatten)]
    shown: Shown,
}

#[derive(Serialize)]
struct Calendar {
    synced: Option<String>,
    messages: usize,
    unresolved: usize,
    days: Vec<Block>,
    pending: Vec<Pending>,
}

/// Serves the calendar on `listen`, reading the data directory `dir` on each request.
///
/// # Errors
///
/// Fails when the address cannot be bound or the server stops with an error.
pub async fn serve(listen: SocketAddr, dir: PathBuf) -> Result<()> {
    let app = axum::Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route(
            "/font.ttf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "font/ttf")],
                    &include_bytes!("../web/HankenGrotesk.ttf")[..],
                )
            }),
        )
        .route("/api/calendar", get(calendar))
        .with_state(dir);
    let listener = tokio::net::TcpListener::bind(listen).await.with_context(|| format!("cannot bind {listen}"))?;
    Ok(axum::serve(listener, app).await?)
}

async fn calendar(State(dir): State<PathBuf>, Query(window): Query<Window>) -> impl IntoResponse {
    within(&dir, &window)
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}")))
}

fn within(dir: &Path, window: &Window) -> Result<Calendar> {
    let board = Board::load(&Store::open(dir)?, dir)?;
    let days = board.days().into_iter().filter(|day| window.first <= day.date && day.date <= window.last);
    let days = days.map(|day| Block {
        day: day.date,
        kind: day.kind,
        part: day.part,
        shown: board.message(&day.message_ts).expect("cached"),
    });
    let mut pending = board.pending();
    let unresolved = pending.len();
    let (first, last) = (window.first.to_string(), window.last.to_string());
    pending.retain(|pending| (first.as_str()..=last.as_str()).contains(&&pending.shown.posted[4..14]));
    Ok(Calendar {
        synced: board.synced.clone(),
        messages: board.len(),
        unresolved,
        days: days.collect(),
        pending,
    })
}
