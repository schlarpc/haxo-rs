// The settings file on the card, through `haxophone-config`: it is the one
// place that knows the format, and it edits the file without touching the
// rest (comments, layout), so the file stays readable and editable by hand.
use std::process::Stdio;

use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

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
    pub password: Option<String>,
}

async fn run(args: &Args, cmd: &[&str], stdin: Option<&str>) -> Result<String> {
    let mut c = Command::new(&args.haxophone_config);
    c.args(cmd)
        .arg("--config")
        .arg(&args.config)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c
        .spawn()
        .map_err(|e| Error::internal(format!("running haxophone-config: {}", e)))?;
    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(input.as_bytes()).await?;
        pipe.write_all(b"\n").await?;
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error::internal(format!(
            "The settings file couldn't be changed: {}",
            if msg.is_empty() {
                "haxophone-config failed"
            } else {
                &msg
            }
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The parsed settings file. A missing or broken file reads as empty.
pub async fn show(args: &Args) -> Result<Shown> {
    let mut c = Command::new(&args.haxophone_config);
    c.arg("show").arg("--config").arg(&args.config);
    let out = c
        .output()
        .await
        .map_err(|e| Error::internal(format!("running haxophone-config: {}", e)))?;
    Ok(serde_json::from_slice(&out.stdout).unwrap_or_default())
}

pub async fn wifi_add(args: &Args, ssid: &str, psk: Option<&str>) -> Result<()> {
    match psk {
        Some(psk) => run(args, &["wifi-add", ssid, "--psk-stdin"], Some(psk)).await?,
        None => run(args, &["wifi-add", ssid], None).await?,
    };
    Ok(())
}

pub async fn wifi_remove(args: &Args, ssid: &str) -> Result<()> {
    run(args, &["wifi-remove", ssid], None).await?;
    Ok(())
}

#[derive(Default)]
pub struct InstrumentChange {
    pub soundfont: Option<String>,
    pub program: Option<i32>,
    pub transpose: Option<i32>,
    pub volume: Option<i32>,
}

pub async fn set_instrument(args: &Args, c: &InstrumentChange) -> Result<()> {
    let mut a: Vec<String> = vec!["set-instrument".into()];
    if let Some(s) = &c.soundfont {
        a.extend(["--soundfont".into(), s.clone()]);
    }
    for (flag, v) in [
        ("--program", c.program),
        ("--transpose", c.transpose),
        ("--volume", c.volume),
    ] {
        if let Some(v) = v {
            a.extend([flag.into(), v.to_string()]);
        }
    }
    if a.len() == 1 {
        return Ok(());
    }
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    run(args, &refs, None).await?;
    Ok(())
}

/// Apply the settings file again (iwd's network files and the rest).
pub async fn reapply(args: &Args) -> Result<()> {
    let out = Command::new("systemctl")
        .args(["restart", &args.config_unit])
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::internal(format!(
            "Applying the settings failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}
