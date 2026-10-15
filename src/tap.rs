// The audio tap: what the synth renders, for listeners (the web UI) on a
// Unix socket (--audio-socket). Protocol in the haxo-control crate: a JSON
// line with the format, then raw s16le PCM.
//
// fluidsynth's audio thread (realtime, feeding the in-ear jack) pushes each
// period into a wait-free ring and never waits: when the ring is full the
// period is dropped. This thread, at normal priority, drains the ring to
// every connected client every few milliseconds. It keeps no backlog:
// what a slow client can't take is dropped, and a delay in this thread is
// caught up by skipping, so listeners hear the instrument late by
// milliseconds, not by however far behind they fell.
use std::io::{ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;
use std::time::Duration;

use haxo_control::AudioFormat;
use log::{info, warn};

const POLL: Duration = Duration::from_millis(5);
// More than this waiting in the ring, and the oldest is skipped.
const MAX_BACKLOG_MS: usize = 40;
// What the ring holds: plenty for a late poll, small next to the backlog
// limit's purpose.
const RING_MS: usize = 250;

pub type Producer = rtrb::Producer<i16>;

// Listeners at once: the web UI, the USB and Bluetooth bridges, and room
// to spare. Each costs this thread a copy per poll.
const MAX_CLIENTS: usize = 8;

struct Client {
    stream: UnixStream,
    // Bytes a short write left behind: sent before anything new, so the
    // stream stays aligned.
    pending: Vec<u8>,
}

impl Client {
    // Send what this poll read; false when the listener is gone. New audio
    // waits for an old remainder; when that can't go, the new is dropped.
    fn send(&mut self, bytes: &[u8]) -> bool {
        if !self.pending.is_empty() {
            match self.stream.write(&self.pending) {
                Ok(n) => drop(self.pending.drain(..n)),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => return false,
            }
        }
        if self.pending.is_empty() && !bytes.is_empty() {
            match self.stream.write(bytes) {
                Ok(n) => self.pending.extend_from_slice(&bytes[n..]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => return false,
            }
        }
        true
    }
}

/// The ring between the audio thread and the tap thread, for `rate`.
pub fn ring(rate: u32) -> (Producer, rtrb::Consumer<i16>) {
    rtrb::RingBuffer::new(rate as usize * RING_MS / 1000)
}

/// Push one period, mono, from the audio thread: wait-free, and dropped
/// whole when the ring is full.
pub fn push(producer: &mut Producer, left: &[f32], right: &[f32]) {
    let Ok(chunk) = producer.write_chunk_uninit(left.len()) else {
        return;
    };
    chunk.fill_from_iter(left.iter().zip(right).map(|(l, r)| {
        // The jack plays the left channel only; listeners get both, summed.
        let s = (l + r) * 0.5;
        (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }));
}

/// Serve the ring's audio on `path` to every client (up to MAX_CLIENTS),
/// each on its own: one that falls behind loses audio, the others don't.
/// Start it before the main loop turns realtime.
pub fn serve(
    path: &str,
    mut ring: rtrb::Consumer<i16>,
    format: AudioFormat,
) -> std::io::Result<()> {
    if Path::new(path).exists() {
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    info!("Audio tap at {} ({} Hz, mono)", path, format.rate);
    let header = serde_json::to_string(&format).unwrap() + "\n";
    let max_backlog = format.rate as usize * MAX_BACKLOG_MS / 1000;
    thread::Builder::new()
        .name("audio-tap".into())
        .spawn(move || {
            let mut clients: Vec<Client> = Vec::new();
            let mut bytes: Vec<u8> = Vec::new();
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if clients.len() >= MAX_CLIENTS {
                            warn!(
                                "Audio tap: {} listeners already, refusing another",
                                MAX_CLIENTS
                            );
                        } else if stream.write_all(header.as_bytes()).is_ok()
                            && stream.set_nonblocking(true).is_ok()
                        {
                            clients.push(Client {
                                stream,
                                pending: Vec::new(),
                            });
                            info!("Audio tap: listener connected ({} now)", clients.len());
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => warn!("audio tap: {}", e),
                }

                let mut waiting = ring.slots();
                if waiting > max_backlog {
                    // Late: skip to the newest.
                    if let Ok(c) = ring.read_chunk(waiting - max_backlog / 2) {
                        c.commit_all();
                    }
                    waiting = ring.slots();
                }
                bytes.clear();
                if let Ok(c) = ring.read_chunk(waiting) {
                    let (a, b) = c.as_slices();
                    for s in a.iter().chain(b) {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                    c.commit_all();
                }

                let before = clients.len();
                clients.retain_mut(|c| c.send(&bytes));
                if clients.len() < before {
                    info!("Audio tap: listener gone ({} now)", clients.len());
                }
                thread::sleep(POLL);
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};

    #[test]
    fn every_listener_gets_everything() {
        let dir = std::env::temp_dir().join(format!("haxo-tap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tap").to_string_lossy().to_string();
        let (mut producer, consumer) = ring(48_000);
        serve(&path, consumer, AudioFormat::s16le(48_000, 1)).unwrap();
        let listeners: Vec<_> = (0..3)
            .map(|_| {
                let s = UnixStream::connect(&path).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                BufReader::new(s)
            })
            .collect();
        // Let the tap thread accept them all before there's audio.
        thread::sleep(POLL * 4);
        let left: Vec<f32> = (0..480).map(|i| (i as f32 / 480.0) - 0.5).collect();
        push(&mut producer, &left, &left);
        for mut l in listeners {
            let mut header = String::new();
            l.read_line(&mut header).unwrap();
            let f: AudioFormat = serde_json::from_str(&header).unwrap();
            assert_eq!(f.rate, 48_000);
            let mut pcm = vec![0u8; 480 * 2];
            l.read_exact(&mut pcm).unwrap();
            let first = i16::from_le_bytes([pcm[0], pcm[1]]);
            let last = i16::from_le_bytes([pcm[958], pcm[959]]);
            assert!(first < -16000 && last > 16000, "{} {}", first, last);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
