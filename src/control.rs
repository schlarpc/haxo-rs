// The control socket (--control-socket): the protocol is in the
// haxo-control crate, shared with its clients. Live state goes out to
// every client when it changes, at most every PUBLISH_INTERVAL (latest
// wins, never queued); `Set` lines come in.
//
// The main loop never waits on this: it publishes with try_lock (skipping a
// tick when a client thread holds the lock) and takes commands with
// try_recv. Each client has its own threads, so a slow one only stalls
// itself.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub use haxo_control::{Live, Set};
use log::{info, warn};

const PUBLISH_INTERVAL: Duration = Duration::from_millis(16);

pub struct Control {
    shared: Arc<Mutex<(u64, Live)>>,
    last: Live,
    rx: mpsc::Receiver<Set>,
}

impl Control {
    // Listen on `path`, replacing a stale socket. Start it before the main
    // loop turns realtime, so the threads don't inherit that.
    pub fn start(path: &str) -> std::io::Result<Self> {
        if Path::new(path).exists() {
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        let shared = Arc::new(Mutex::new((0u64, Live::default())));
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => client(stream, s.clone(), tx.clone()),
                        Err(e) => warn!("control socket: {}", e),
                    }
                }
            })?;
        info!("Control socket at {}", path);
        Ok(Control {
            shared,
            last: Live::default(),
            rx,
        })
    }

    // Call every tick with the current state.
    pub fn publish(&mut self, live: &Live) {
        if *live == self.last {
            return;
        }
        if let Ok(mut g) = self.shared.try_lock() {
            g.0 += 1;
            g.1 = live.clone();
            self.last = live.clone();
        }
    }

    pub fn commands(&self) -> mpsc::TryIter<'_, Set> {
        self.rx.try_iter()
    }
}

fn client(stream: UnixStream, shared: Arc<Mutex<(u64, Live)>>, tx: mpsc::Sender<Set>) {
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    // Commands in.
    let _ = thread::Builder::new()
        .name("control-in".into())
        .spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match Set::parse(&line) {
                    Ok(set) => {
                        if tx.send(set).is_err() {
                            return;
                        }
                    }
                    Err(e) => warn!("control socket: {}: {}", e, line),
                }
            }
        });
    // State out.
    let _ = thread::Builder::new()
        .name("control-out".into())
        .spawn(move || {
            let mut stream = stream;
            let mut seen = u64::MAX;
            loop {
                let snapshot = {
                    let g = shared.lock().unwrap();
                    if g.0 == seen {
                        None
                    } else {
                        seen = g.0;
                        Some(g.1.to_line())
                    }
                };
                if let Some(s) = snapshot {
                    if stream.write_all(s.as_bytes()).is_err() {
                        break;
                    }
                }
                thread::sleep(PUBLISH_INTERVAL);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_state_and_takes_commands() {
        use std::io::BufRead;
        let dir = std::env::temp_dir().join(format!("haxo-control-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s").to_string_lossy().to_string();
        let mut control = Control::start(&path).unwrap();
        let mut c = UnixStream::connect(&path).unwrap();
        let live = Live {
            keys: 9344,
            breath: 40,
            note: Some(67),
            program: 66,
            ..Default::default()
        };
        control.publish(&live);
        let mut line = String::new();
        BufReader::new(c.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let got: Live = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(got, live);
        writeln!(c, r#"{{"program":5}}"#).unwrap();
        let mut got = None;
        for _ in 0..100 {
            if let Some(cmd) = control.commands().next() {
                got = Some(cmd);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.and_then(|s| s.program), Some(5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
