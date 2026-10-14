// Soundfonts and the instrument's settings. Uploaded files live in
// --files-dir (also the MTP storage, so a computer on the USB cable sees
// them); the built-in ones come with the system. Changes go to haxo001
// through its control socket, and into the settings file so they hold
// after a reboot.
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::Json;
use futures::StreamExt;
use haxo_control::Set;
use log::info;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::settings::{self, InstrumentChange};
use crate::Args;

// GM tenor sax (zero-based), what plays when the soundfont in use goes.
const DEFAULT_PROGRAM: i32 = 66;
const MAX_UPLOAD: u64 = 1 << 30;

#[derive(Serialize)]
pub struct File {
    name: String,
    size: u64,
    presets: usize,
    builtin: bool,
}

#[derive(Serialize)]
pub struct ProgramName {
    program: u16,
    name: String,
}

#[derive(Serialize)]
pub struct Storage {
    used: u64,
    total: u64,
}

#[derive(Serialize)]
pub struct Status {
    files: Vec<File>,
    active: Option<String>,
    preset: Option<i32>,
    /// Bank 0 of the active soundfont, by program number.
    presets: Vec<ProgramName>,
    transpose: Option<i32>,
    volume: Option<i32>,
    storage: Storage,
    /// Whether haxo001 is running (the settings are only live then).
    running: bool,
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_soundfont(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".sf2") || l.ends_with(".sf3")
}

fn valid_upload_name(name: &str) -> bool {
    is_soundfont(name)
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\0')
        && name.len() <= 200
}

/// Every soundfont, built in first: (name, path, built in).
fn all(args: &Args) -> Vec<(String, PathBuf, bool)> {
    let mut out: Vec<(String, PathBuf, bool)> = args
        .builtin_soundfonts
        .iter()
        .map(|p| (file_name(p), p.clone(), true))
        .collect();
    if let Ok(dir) = std::fs::read_dir(&args.files_dir) {
        let mut uploaded: Vec<_> = dir
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| valid_upload_name(n) && !out.iter().any(|(b, _, _)| b == n))
            .collect();
        uploaded.sort_by_key(|n| n.to_lowercase());
        out.extend(
            uploaded
                .into_iter()
                .map(|n| (n.clone(), args.files_dir.join(&n), false)),
        );
    }
    out
}

fn resolve(args: &Args, name: &str) -> Result<(PathBuf, bool)> {
    all(args)
        .into_iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, p, b)| (p, b))
        .ok_or_else(|| Error::not_found(format!("There's no soundfont called {}.", name)))
}

fn storage(dir: &Path) -> Storage {
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
        return Storage { used: 0, total: 0 };
    };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Storage { used: 0, total: 0 };
    }
    let frsize = st.f_frsize as u64;
    Storage {
        used: (st.f_blocks as u64 - st.f_bfree as u64) * frsize,
        total: st.f_blocks as u64 * frsize,
    }
}

async fn build_status(app: &crate::App) -> Result<Status> {
    let args = &app.args;
    let live = app.live.current();
    let files: Vec<File> = all(args)
        .into_iter()
        .map(|(name, path, builtin)| File {
            size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            presets: app.sf2.get(&path).map(|p| p.len()).unwrap_or(0),
            name,
            builtin,
        })
        .collect();
    let active = match &live {
        Some(l) => Some(file_name(Path::new(&l.soundfont))),
        None => settings::show(args).await?.instrument.soundfont,
    };
    let presets = match active.as_deref().and_then(|a| resolve(args, a).ok()) {
        Some((path, _)) => app
            .sf2
            .get(&path)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.bank == 0)
            .map(|p| ProgramName {
                program: p.program,
                name: p.name,
            })
            .collect(),
        None => Vec::new(),
    };
    Ok(Status {
        files,
        active,
        preset: live.as_ref().map(|l| l.program),
        presets,
        transpose: live.as_ref().map(|l| l.transpose),
        volume: live.as_ref().map(|l| l.volume),
        storage: storage(&args.files_dir),
        running: live.is_some(),
    })
}

/// `GET /api/soundfonts`
pub async fn status(State(app): State<crate::State>) -> Result<Json<Status>> {
    Ok(Json(build_status(&app).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    active: Option<String>,
    preset: Option<i32>,
    transpose: Option<i32>,
    volume: Option<i32>,
}

async fn apply(app: &crate::App, u: Update) -> Result<Status> {
    let args = &app.args;
    let soundfont = match &u.active {
        Some(name) => Some(resolve(args, name)?.0),
        None => None,
    };
    let set = Set {
        program: u.preset,
        transpose: u.transpose,
        volume: u.volume,
        soundfont: soundfont.as_ref().map(|p| p.to_string_lossy().into_owned()),
    };
    set.check().map_err(Error::bad_request)?;
    app.live.send(set).await?;
    settings::set_instrument(
        args,
        &InstrumentChange {
            soundfont: u.active.clone(),
            program: u.preset,
            transpose: u.transpose,
            volume: u.volume,
        },
    )
    .await?;
    // A new soundfont takes a while to load; answer once it plays.
    let wanted = soundfont.map(|p| p.to_string_lossy().into_owned());
    for _ in 0..if wanted.is_some() { 300 } else { 5 } {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let live = app.live.current();
        let ok = match (&wanted, &live) {
            (Some(w), Some(l)) => &l.soundfont == w,
            _ => true,
        };
        if ok && wanted.is_some() {
            break;
        }
    }
    build_status(app).await
}

/// `PATCH /api/instrument` `{ active?, preset?, transpose?, volume? }`
pub async fn update(
    State(app): State<crate::State>,
    Json(u): Json<Update>,
) -> Result<Json<Status>> {
    Ok(Json(apply(&app, u).await?))
}

/// `PUT /api/soundfonts/{name}`: the file as the body.
pub async fn upload(
    State(app): State<crate::State>,
    UrlPath(name): UrlPath<String>,
    body: Body,
) -> Result<Json<Status>> {
    let args = &app.args;
    if !valid_upload_name(&name) {
        return Err(Error::bad_request(
            "Only .sf2 and .sf3 soundfonts, with a plain file name.",
        ));
    }
    if args.builtin_soundfonts.iter().any(|p| file_name(p) == name) {
        return Err(Error::bad_request(format!(
            "{} is built in; rename the file to upload it.",
            name
        )));
    }
    tokio::fs::create_dir_all(&args.files_dir).await?;
    let part = args.files_dir.join(format!(".{}.part", name));
    let result = async {
        let mut out = tokio::fs::File::create(&part).await?;
        let mut stream = body.into_data_stream();
        let mut size = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| Error::bad_request(format!("Upload broke off: {}", e)))?;
            size += chunk.len() as u64;
            if size > MAX_UPLOAD {
                return Err(Error::bad_request("That file is over 1 GB."));
            }
            out.write_all(&chunk).await?;
        }
        out.sync_all().await?;
        crate::sf2::presets(&part)
            .map_err(|e| Error::bad_request(format!("{} isn't a soundfont: {}", name, e)))?;
        Ok(size)
    }
    .await;
    match result {
        Ok(size) => {
            tokio::fs::rename(&part, args.files_dir.join(&name)).await?;
            info!("Uploaded {} ({} bytes)", name, size);
            Ok(Json(build_status(&app).await?))
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

/// `DELETE /api/soundfonts/{name}`: not for built-in ones. When it's the
/// one playing, the first built-in one takes over.
pub async fn remove(
    State(app): State<crate::State>,
    UrlPath(name): UrlPath<String>,
) -> Result<Json<Status>> {
    let args = &app.args;
    let (path, builtin) = resolve(args, &name)?;
    if builtin {
        return Err(Error::bad_request(format!("{} is built in.", name)));
    }
    let playing = app
        .live
        .current()
        .map(|l| Path::new(&l.soundfont) == path)
        .unwrap_or(false);
    let saved = settings::show(args).await?.instrument.soundfont;
    if playing || saved.as_deref() == Some(name.as_str()) {
        let Some(fallback) = args.builtin_soundfonts.first() else {
            return Err(Error::bad_request(
                "It's in use, and there's no built-in soundfont to switch to.",
            ));
        };
        let update = Update {
            active: Some(file_name(fallback)),
            preset: Some(DEFAULT_PROGRAM),
            transpose: None,
            volume: None,
        };
        if playing {
            apply(&app, update).await?;
        } else {
            settings::set_instrument(
                args,
                &InstrumentChange {
                    soundfont: update.active,
                    program: update.preset,
                    ..Default::default()
                },
            )
            .await?;
        }
    }
    tokio::fs::remove_file(&path).await?;
    info!("Deleted {}", name);
    Ok(Json(build_status(&app).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_names() {
        assert!(valid_upload_name("Mister_Tenor_Sax.sf2"));
        assert!(valid_upload_name("x.SF3"));
        assert!(!valid_upload_name("../x.sf2"));
        assert!(!valid_upload_name(".hidden.sf2"));
        assert!(!valid_upload_name("x.wav"));
        assert!(!valid_upload_name("a/b.sf2"));
    }
}
