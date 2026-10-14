// Preset names from a soundfont (SF2, and SF3, which differs only in its
// samples): the `phdr` chunk in the `pdta` list. The sample data, nearly
// all of the file, is skipped over, never read.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preset {
    pub bank: u16,
    pub program: u16,
    pub name: String,
}

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The presets of the soundfont at `path`, sorted by bank and program.
pub fn presets(path: &Path) -> std::io::Result<Vec<Preset>> {
    let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"sfbk" {
        return Err(bad("not a soundfont (no RIFF sfbk header)"));
    }
    let end = 8 + u32le(&head[4..8]) as u64;
    let mut pos = 12u64;
    // Top level: LIST chunks (INFO, sdta, pdta).
    while pos + 12 <= end {
        f.seek(SeekFrom::Start(pos))?;
        let mut ch = [0u8; 12];
        f.read_exact(&mut ch)?;
        let size = u32le(&ch[4..8]) as u64;
        if &ch[0..4] == b"LIST" && &ch[8..12] == b"pdta" {
            return phdr(&mut f, pos + 12, pos + 8 + size);
        }
        pos += 8 + size + (size & 1);
    }
    Err(bad("no preset data (pdta) in the soundfont"))
}

fn phdr(f: &mut std::fs::File, mut pos: u64, end: u64) -> std::io::Result<Vec<Preset>> {
    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos))?;
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch)?;
        let size = u32le(&ch[4..8]) as u64;
        if &ch[0..4] == b"phdr" {
            let mut data = vec![0u8; size.min(1 << 20) as usize];
            f.read_exact(&mut data)?;
            let mut out: Vec<Preset> = data
                .chunks_exact(38)
                .map(|r| Preset {
                    name: String::from_utf8_lossy(&r[0..20])
                        .trim_end_matches('\0')
                        .split('\0')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    program: u16::from_le_bytes([r[20], r[21]]),
                    bank: u16::from_le_bytes([r[22], r[23]]),
                })
                .collect();
            // The last record is the terminal "EOP".
            out.pop();
            out.sort_by_key(|p| (p.bank, p.program));
            return Ok(out);
        }
        pos += 8 + size + (size & 1);
    }
    Ok(Vec::new())
}

/// Parsed presets by file, until the file changes.
#[derive(Default)]
pub struct Cache {
    map: Mutex<HashMap<PathBuf, (u64, SystemTime, Vec<Preset>)>>,
}

impl Cache {
    pub fn get(&self, path: &Path) -> std::io::Result<Vec<Preset>> {
        let meta = std::fs::metadata(path)?;
        let key = (meta.len(), meta.modified()?);
        if let Some((len, mtime, p)) = self.map.lock().unwrap().get(path) {
            if (*len, *mtime) == key {
                return Ok(p.clone());
            }
        }
        let p = presets(path)?;
        self.map
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), (key.0, key.1, p.clone()));
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((data.len() as u32).to_le_bytes());
        v.extend(data);
        if data.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn record(name: &str, program: u16, bank: u16) -> Vec<u8> {
        let mut r = vec![0u8; 38];
        r[..name.len()].copy_from_slice(name.as_bytes());
        r[20..22].copy_from_slice(&program.to_le_bytes());
        r[22..24].copy_from_slice(&bank.to_le_bytes());
        r
    }

    #[test]
    fn reads_presets_skipping_samples() {
        let mut phdr = record("Tenor Sax", 66, 0);
        phdr.extend(record("Standard", 0, 128));
        phdr.extend(record("Piano", 0, 0));
        phdr.extend(record("EOP", 0, 0));
        let mut pdta = b"pdta".to_vec();
        pdta.extend(chunk(b"phdr", &phdr));
        let mut sdta = b"sdta".to_vec();
        sdta.extend(chunk(b"smpl", &[1u8; 1001]));
        let mut body = b"sfbk".to_vec();
        body.extend(chunk(b"LIST", b"INFOxx"));
        body.extend(chunk(b"LIST", &sdta));
        body.extend(chunk(b"LIST", &pdta));
        let file = chunk(b"RIFF", &body);
        let path = std::env::temp_dir().join(format!("haxo-sf2-{}.sf2", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let p = presets(&path).unwrap();
        let names: Vec<(u16, u16, &str)> = p
            .iter()
            .map(|p| (p.bank, p.program, p.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![(0, 0, "Piano"), (0, 66, "Tenor Sax"), (128, 0, "Standard")]
        );
        std::fs::write(&path, b"not a soundfont").unwrap();
        assert!(presets(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    // A real soundfont, when HAXO_TEST_SF2 points at one.
    #[test]
    fn reads_a_real_soundfont() {
        let Ok(path) = std::env::var("HAXO_TEST_SF2") else {
            return;
        };
        let p = presets(Path::new(&path)).unwrap();
        let bank0 = p.iter().filter(|p| p.bank == 0).count();
        println!(
            "{} presets, {} in bank 0, e.g. {:?}",
            p.len(),
            bank0,
            p.get(66)
        );
        assert!(bank0 > 0);
    }
}
