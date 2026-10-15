// The audio tap: what the synth renders, for listeners (the web UI) on a
// Unix socket (--audio-socket). Protocol in the haxo-control crate: a JSON
// line with the format, then raw s16le PCM.
//
// fluidsynth's audio thread (realtime, feeding the in-ear jack) pushes each
// period into a wait-free ring and never waits: when the ring is full the
// period is dropped. This thread, at normal priority, drains the ring to
// the one connected client every few milliseconds. It keeps no backlog:
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

/// Serve the ring's audio on `path` to one client at a time (a new client
/// takes over). Start it before the main loop turns realtime.
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
            let mut client: Option<UnixStream> = None;
            // Bytes of a sample a short write left behind: sent before
            // anything new, so the stream stays aligned.
            let mut pending: Vec<u8> = Vec::new();
            let mut bytes: Vec<u8> = Vec::new();
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if stream.write_all(header.as_bytes()).is_ok()
                            && stream.set_nonblocking(true).is_ok()
                        {
                            info!("Audio tap: listener connected");
                            client = Some(stream);
                            pending.clear();
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

                if let Some(stream) = client.as_mut() {
                    let mut gone = false;
                    // The remainder of a short write first, then new audio
                    // only once that's gone; otherwise the new is dropped.
                    if !pending.is_empty() {
                        match stream.write(&pending) {
                            Ok(n) => drop(pending.drain(..n)),
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                            Err(_) => gone = true,
                        }
                    }
                    if !gone && pending.is_empty() && !bytes.is_empty() {
                        match stream.write(&bytes) {
                            Ok(n) => pending.extend_from_slice(&bytes[n..]),
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                            Err(_) => gone = true,
                        }
                    }
                    if gone {
                        info!("Audio tap: listener gone");
                        client = None;
                        pending.clear();
                    }
                }
                thread::sleep(POLL);
            }
        })?;
    Ok(())
}
