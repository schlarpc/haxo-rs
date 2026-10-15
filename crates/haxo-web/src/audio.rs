// The instrument's sound for listeners: one connection to haxo001's audio
// tap (--audio-socket), split 1:N. Each chunk goes to every listener
// through a broadcast channel; one that can't keep up skips ahead instead
// of queueing, so nobody hears the instrument later than the network
// makes them.
//
// - `WS /api/audio`: the format as a JSON text frame, then binary s16le.
//   The page plays it with a small jitter buffer.
// - `GET /api/stream.wav`: an endless WAV over HTTP (the sizes in its
//   header say "unknown"), for ffmpeg, VLC, mpv and the like:
//   `ffplay -fflags nobuffer -flags low_delay http://haxophone.local/api/stream.wav`.
use std::path::PathBuf;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use haxo_control::AudioFormat;
use log::{info, warn};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, watch};

use crate::error::{Error, Result};

// Chunks a listener may fall behind by before it skips: the tap sends one
// about every 5 ms.
const LAG_CHUNKS: usize = 16;

pub struct Audio {
    format: watch::Receiver<Option<AudioFormat>>,
    chunks: broadcast::Sender<Bytes>,
}

impl Audio {
    pub fn start(socket: PathBuf) -> Self {
        let (format_tx, format) = watch::channel(None);
        let (chunks, _) = broadcast::channel(LAG_CHUNKS);
        tokio::spawn(run(socket, format_tx, chunks.clone()));
        Audio { format, chunks }
    }
}

async fn run(
    socket: PathBuf,
    format: watch::Sender<Option<AudioFormat>>,
    chunks: broadcast::Sender<Bytes>,
) {
    let mut logged = false;
    loop {
        match UnixStream::connect(&socket).await {
            Ok(stream) => {
                logged = false;
                if let Err(e) = read(stream, &format, &chunks).await {
                    warn!("audio tap: {}", e);
                }
                info!("Audio tap closed");
            }
            Err(e) => {
                if !logged {
                    warn!("audio tap {}: {}", socket.display(), e);
                    logged = true;
                }
            }
        }
        format.send_replace(None);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn read(
    stream: UnixStream,
    format: &watch::Sender<Option<AudioFormat>>,
    chunks: &broadcast::Sender<Bytes>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let f: AudioFormat = serde_json::from_str(&line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    info!("Audio tap: {} Hz, {} channel(s)", f.rate, f.channels);
    let frame = f.bytes_per_frame();
    format.send_replace(Some(f));
    let mut buf = vec![0u8; 8192];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        // Whole frames only, so every listener stays aligned.
        carry.extend_from_slice(&buf[..n]);
        let whole = carry.len() / frame * frame;
        if whole > 0 {
            let chunk = Bytes::copy_from_slice(&carry[..whole]);
            carry.drain(..whole);
            // No listeners is fine.
            let _ = chunks.send(chunk);
        }
    }
}

fn unavailable() -> Error {
    Error::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The instrument isn't playing.",
    )
}

/// `WS /api/audio`
pub async fn websocket(State(app): State<crate::State>, ws: WebSocketUpgrade) -> Response {
    let format = app.audio.format.clone();
    let chunks = app.audio.chunks.subscribe();
    ws.on_upgrade(move |socket| stream_ws(socket, format, chunks))
}

async fn stream_ws(
    mut socket: WebSocket,
    mut format: watch::Receiver<Option<AudioFormat>>,
    mut chunks: broadcast::Receiver<Bytes>,
) {
    format.mark_changed();
    loop {
        tokio::select! {
            changed = format.changed() => {
                if changed.is_err() { return }
                // A new format (haxo restarted), or none: tell the page.
                let text = serde_json::to_string(&*format.borrow_and_update()).unwrap();
                if socket.send(Message::text(text)).await.is_err() { return }
            }
            chunk = chunks.recv() => match chunk {
                Ok(c) => if socket.send(Message::Binary(c)).await.is_err() { return },
                // Fell behind: skip what was missed.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return,
            },
        }
    }
}

/// A WAV header for a stream of unknown length: ffmpeg and the rest take
/// 0xFFFFFFFF sizes as "until the connection ends".
pub fn wav_header(f: &AudioFormat) -> Vec<u8> {
    let channels = f.channels as u32;
    let block = 2 * channels;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&(f.channels).to_le_bytes());
    h.extend_from_slice(&f.rate.to_le_bytes());
    h.extend_from_slice(&(f.rate * block).to_le_bytes());
    h.extend_from_slice(&(block as u16).to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h
}

/// `GET /api/stream.wav`
pub async fn wav(State(app): State<crate::State>) -> Result<Response> {
    let Some(format) = app.audio.format.borrow().clone() else {
        return Err(unavailable());
    };
    let header = Bytes::from(wav_header(&format));
    let chunks = app.audio.chunks.subscribe();
    let mut formats = app.audio.format.clone();
    formats.mark_unchanged();
    // A WAV can't change format midway: the stream ends when haxo goes
    // (and maybe comes back different), and the player reconnects.
    let body = futures::stream::unfold(
        (Some(header), chunks, formats),
        |(header, mut chunks, mut formats)| async move {
            if let Some(h) = header {
                return Some((Ok::<_, std::io::Error>(h), (None, chunks, formats)));
            }
            loop {
                tokio::select! {
                    _ = formats.changed() => return None,
                    chunk = chunks.recv() => match chunk {
                        Ok(c) => return Some((Ok(c), (None, chunks, formats))),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    },
                }
            }
        },
    );
    Ok((
        [
            (header::CONTENT_TYPE, "audio/wav"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(body),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_44_bytes_of_pcm() {
        let h = wav_header(&AudioFormat::s16le(44100, 1));
        assert_eq!(h.len(), 44);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 44100);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 88200);
        assert_eq!(&h[36..40], b"data");
    }
}
