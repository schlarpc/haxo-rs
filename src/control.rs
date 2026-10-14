// A control socket for the web UI (or anything else on the board): JSON
// lines over a Unix stream socket.
//
// Out, to every client: the live state, as one object per line, when it
// changes and at most every PUBLISH_INTERVAL (latest wins, never queued):
//   {"keys":9344,"breath":87,"note":67,"program":66,"transpose":-14,
//    "volume":100,"soundfont":"/path/to.sf2"}
// In, one command per line, any of these fields:
//   {"program":66} {"transpose":-14} {"volume":80}
//   {"soundfont":"/path/to.sf2","program":0}
//
// The main loop never waits on this: it publishes with try_lock (skipping a
// tick when a client thread holds the lock) and takes commands with
// try_recv. Each client has its own thread, so a slow one only stalls itself.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use log::{info, warn};
use serde_json::{json, Value};

const PUBLISH_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub keys: u32,
    pub breath: i32,
    pub note: Option<i32>,
    pub program: i32,
    pub transpose: i32,
    pub volume: i32,
    pub soundfont: String,
}

impl Live {
    fn to_json(&self) -> String {
        json!({
            "keys": self.keys,
            "breath": self.breath,
            "note": self.note,
            "program": self.program,
            "transpose": self.transpose,
            "volume": self.volume,
            "soundfont": self.soundfont,
        })
        .to_string()
    }
}

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Program(i32),
    Transpose(i32),
    Volume(i32),
    Soundfont(String, Option<i32>),
}

// The commands in one line; unknown fields and bad values are skipped.
pub fn parse(line: &str) -> Result<Vec<Cmd>, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let obj = v.as_object().ok_or("not a JSON object")?;
    let int = |k: &str, lo: i64, hi: i64| -> Result<Option<i32>, String> {
        match obj.get(k) {
            None => Ok(None),
            Some(x) => match x.as_i64() {
                Some(n) if (lo..=hi).contains(&n) => Ok(Some(n as i32)),
                _ => Err(format!("{} must be an integer from {} to {}", k, lo, hi)),
            },
        }
    };
    let mut cmds = Vec::new();
    let program = int("program", 0, 127)?;
    if let Some(sf) = obj.get("soundfont") {
        let path = sf.as_str().ok_or("soundfont must be a path")?;
        cmds.push(Cmd::Soundfont(path.to_string(), program));
    } else if let Some(p) = program {
        cmds.push(Cmd::Program(p));
    }
    if let Some(t) = int("transpose", -48, 48)? {
        cmds.push(Cmd::Transpose(t));
    }
    if let Some(v) = int("volume", 0, 100)? {
        cmds.push(Cmd::Volume(v));
    }
    Ok(cmds)
}

pub struct Control {
    shared: Arc<Mutex<(u64, Live)>>,
    last: Live,
    rx: mpsc::Receiver<Cmd>,
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

    pub fn commands(&self) -> mpsc::TryIter<'_, Cmd> {
        self.rx.try_iter()
    }
}

fn client(stream: UnixStream, shared: Arc<Mutex<(u64, Live)>>, tx: mpsc::Sender<Cmd>) {
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
                match parse(&line) {
                    Ok(cmds) => {
                        for c in cmds {
                            if tx.send(c).is_err() {
                                return;
                            }
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
                        Some(g.1.to_json())
                    }
                };
                if let Some(s) = snapshot {
                    if writeln!(stream, "{}", s).is_err() {
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
    fn parses_commands() {
        assert_eq!(parse(r#"{"program":66}"#).unwrap(), vec![Cmd::Program(66)]);
        assert_eq!(
            parse(r#"{"transpose":-14,"volume":80}"#).unwrap(),
            vec![Cmd::Transpose(-14), Cmd::Volume(80)]
        );
        assert_eq!(
            parse(r#"{"soundfont":"/a.sf2","program":3}"#).unwrap(),
            vec![Cmd::Soundfont("/a.sf2".into(), Some(3))]
        );
        assert_eq!(parse(r#"{"other":1}"#).unwrap(), vec![]);
        assert!(parse(r#"{"program":200}"#).is_err());
        assert!(parse(r#"{"volume":"loud"}"#).is_err());
        assert!(parse("[1]").is_err());
        assert!(parse("nope").is_err());
    }

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
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["keys"], 9344);
        assert_eq!(v["note"], 67);
        writeln!(c, r#"{{"program":5}}"#).unwrap();
        let mut got = None;
        for _ in 0..100 {
            if let Some(cmd) = control.commands().next() {
                got = Some(cmd);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, Some(Cmd::Program(5)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
