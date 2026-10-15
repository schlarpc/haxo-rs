// The settings file on the card, through `haxophone-config serve`: a root
// helper behind a Unix socket (one JSON request per connection), the one
// place that knows the format. It edits the file without touching the rest
// (comments, layout), and the web password never leaves it. haxo-web
// itself runs unprivileged and can't write the FAT partition.
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::{Error, Result};
use crate::Args;

#[derive(Debug, Default, Deserialize)]
pub struct Shown {
    #[serde(default)]
    pub wifi: Vec<SavedWifi>,
    #[serde(default)]
    pub instrument: Instrument,
    #[serde(default)]
    pub web: Web,
}

#[derive(Debug, Deserialize)]
pub struct SavedWifi {
    pub ssid: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Instrument {
    pub soundfont: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Web {
    #[serde(default)]
    pub password_set: bool,
}

// Edits read the file, change it and write it back. The helper locks, but
// a turn here also keeps them in the order they were asked for.
static EDITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn request(args: &Args, req: Value) -> Result<Value> {
    let unavailable = |e: std::io::Error| {
        Error::internal(format!(
            "The settings file can't be reached ({}): {}",
            args.settings_socket.display(),
            e
        ))
    };
    let mut stream = UnixStream::connect(&args.settings_socket)
        .await
        .map_err(unavailable)?;
    let mut line = req.to_string();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .await
        .map_err(unavailable)?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .await
        .map_err(unavailable)?;
    let v: Value = serde_json::from_str(&reply)
        .map_err(|e| Error::internal(format!("The settings helper answered {:?}: {}", reply, e)))?;
    if v["ok"] == Value::Bool(true) {
        Ok(v["result"].clone())
    } else {
        Err(Error::internal(format!(
            "The settings file couldn't be changed: {}",
            v["error"].as_str().unwrap_or("unknown error")
        )))
    }
}

async fn edit(args: &Args, req: Value) -> Result<()> {
    let _turn = EDITING.lock().await;
    request(args, req).await.map(|_| ())
}

/// The parsed settings file. A missing or broken file reads as empty.
pub async fn show(args: &Args) -> Result<Shown> {
    let v = request(args, json!({ "cmd": "show" })).await?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// Whether a web password is set, and whether `given` is it.
pub async fn check_password(args: &Args, given: &str) -> Result<(bool, bool)> {
    let v = request(args, json!({ "cmd": "check-password", "password": given })).await?;
    Ok((
        v["set"] == Value::Bool(true),
        v["match"] == Value::Bool(true),
    ))
}

pub async fn wifi_add(args: &Args, ssid: &str, psk: Option<&str>) -> Result<()> {
    edit(args, json!({ "cmd": "wifi-add", "ssid": ssid, "psk": psk })).await
}

pub async fn wifi_remove(args: &Args, ssid: &str) -> Result<()> {
    edit(args, json!({ "cmd": "wifi-remove", "ssid": ssid })).await
}

#[derive(Default)]
pub struct InstrumentChange {
    pub soundfont: Option<String>,
    pub program: Option<i32>,
    pub transpose: Option<i32>,
    pub volume: Option<i32>,
}

pub async fn set_instrument(args: &Args, c: &InstrumentChange) -> Result<()> {
    if c.soundfont.is_none() && c.program.is_none() && c.transpose.is_none() && c.volume.is_none() {
        return Ok(());
    }
    edit(
        args,
        json!({
            "cmd": "set-instrument",
            "soundfont": c.soundfont,
            "program": c.program,
            "transpose": c.transpose,
            "volume": c.volume,
        }),
    )
    .await
}

/// Apply the settings file again (iwd's network files and the rest).
pub async fn reapply(args: &Args) -> Result<()> {
    edit(args, json!({ "cmd": "apply" })).await
}
