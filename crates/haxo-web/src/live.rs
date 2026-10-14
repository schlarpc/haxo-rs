// The connection to haxo001's control socket: the latest live state (a
// watch channel, so a slow page skips states instead of queueing them),
// settings changes back the other way, and `WS /api/live`.
use std::path::PathBuf;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use haxo_control::{Live as LiveState, Set};
use log::{info, warn};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, watch};

use crate::error::{Error, Result};

// The page redraws at screen rate; more than this is wasted on the link.
const SEND_INTERVAL: Duration = Duration::from_millis(16);

pub struct Live {
    state: watch::Receiver<Option<LiveState>>,
    set: mpsc::Sender<Set>,
}

impl Live {
    pub fn start(socket: PathBuf) -> Self {
        let (state_tx, state) = watch::channel(None);
        let (set, set_rx) = mpsc::channel(16);
        tokio::spawn(run(socket, state_tx, set_rx));
        Live { state, set }
    }

    /// The latest state, if haxo001 is running.
    pub fn current(&self) -> Option<LiveState> {
        self.state.borrow().clone()
    }

    pub async fn send(&self, set: Set) -> Result<()> {
        set.check().map_err(Error::bad_request)?;
        if self.state.borrow().is_none() {
            return Err(Error::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "The instrument isn't running.",
            ));
        }
        self.set
            .send(set)
            .await
            .map_err(|_| Error::internal("control connection gone"))
    }
}

async fn run(
    socket: PathBuf,
    state: watch::Sender<Option<LiveState>>,
    mut set_rx: mpsc::Receiver<Set>,
) {
    let mut logged = false;
    loop {
        let stream = match UnixStream::connect(&socket).await {
            Ok(s) => s,
            Err(e) => {
                if !logged {
                    warn!("haxo control socket {}: {}", socket.display(), e);
                    logged = true;
                }
                state.send_replace(None);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        info!("Connected to haxo at {}", socket.display());
        logged = false;
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => match serde_json::from_str::<LiveState>(&line) {
                        Ok(live) => { state.send_replace(Some(live)); }
                        Err(e) => warn!("haxo sent {:?}: {}", line, e),
                    },
                    _ => break,
                },
                set = set_rx.recv() => match set {
                    Some(set) => {
                        if write.write_all(set.to_line().as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    None => return,
                },
            }
        }
        info!("haxo control socket closed");
        state.send_replace(None);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// `WS /api/live`: the live state as JSON text frames, `null` while the
/// instrument isn't running.
pub async fn websocket(State(app): State<crate::State>, ws: WebSocketUpgrade) -> Response {
    let state = app.live.state.clone();
    ws.on_upgrade(move |socket| stream(socket, state))
}

async fn stream(mut socket: WebSocket, mut state: watch::Receiver<Option<LiveState>>) {
    loop {
        let text = serde_json::to_string(&*state.borrow_and_update()).unwrap();
        if socket.send(Message::text(text)).await.is_err() {
            return;
        }
        tokio::time::sleep(SEND_INTERVAL).await;
        tokio::select! {
            changed = state.changed() => if changed.is_err() { return },
            msg = socket.recv() => match msg {
                // The page sends nothing; anything else is a close.
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return,
            },
        }
    }
}
