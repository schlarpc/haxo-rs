//! The protocol of haxo001's control socket (`--control-socket PATH`):
//! JSON lines over a Unix stream socket.
//!
//! haxo001 sends [`Live`] to every client when it changes, at most every
//! 16 ms (latest wins, never queued). Clients send [`Set`] lines; fields
//! left out are left alone.

use serde::{Deserialize, Serialize};

/// What the instrument is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Live {
    /// The scanned key bitmask (bit = row * 3 + col).
    pub keys: u32,
    /// Breath, 0-127 (the CC2 value).
    pub breath: i32,
    /// The MIDI note sounding, if any.
    pub note: Option<i32>,
    pub program: i32,
    pub transpose: i32,
    /// 0-100, the synth's gain (100 is a gain of 1).
    pub volume: i32,
    /// Path of the soundfont playing.
    pub soundfont: String,
}

/// A change of settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transpose: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<i32>,
    /// Load this soundfont, then select `program` (or keep the current one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soundfont: Option<String>,
}

/// The audio socket (`--audio-socket PATH`): this as one JSON line, then
/// the instrument's sound as raw interleaved PCM, from when the client
/// connects, for as long as it reads. A client that falls behind loses
/// audio rather than delaying it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub rate: u32,
    pub channels: u16,
    /// Always "s16le".
    pub format: String,
}

impl AudioFormat {
    pub fn s16le(rate: u32, channels: u16) -> Self {
        AudioFormat {
            rate,
            channels,
            format: "s16le".into(),
        }
    }

    pub fn bytes_per_frame(&self) -> usize {
        2 * self.channels as usize
    }
}

pub const TRANSPOSE_RANGE: std::ops::RangeInclusive<i32> = -48..=48;

impl Set {
    /// Parse one line and check its ranges.
    pub fn parse(line: &str) -> Result<Set, String> {
        let set: Set = serde_json::from_str(line).map_err(|e| e.to_string())?;
        set.check()?;
        Ok(set)
    }

    pub fn check(&self) -> Result<(), String> {
        let within = |name: &str, v: Option<i32>, r: std::ops::RangeInclusive<i32>| match v {
            Some(n) if !r.contains(&n) => Err(format!(
                "{} must be from {} to {}",
                name,
                r.start(),
                r.end()
            )),
            _ => Ok(()),
        };
        within("program", self.program, 0..=127)?;
        within("transpose", self.transpose, TRANSPOSE_RANGE)?;
        within("volume", self.volume, 0..=100)?;
        if matches!(&self.soundfont, Some(p) if p.is_empty()) {
            return Err("soundfont must be a path".into());
        }
        Ok(())
    }

    /// One line to send, newline included.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap() + "\n"
    }
}

impl Live {
    /// One line to send, newline included.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap() + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_checks_sets() {
        assert_eq!(
            Set::parse(r#"{"program":66}"#).unwrap(),
            Set {
                program: Some(66),
                ..Default::default()
            }
        );
        let s = Set::parse(r#"{"soundfont":"/a.sf2","program":3,"volume":80}"#).unwrap();
        assert_eq!(s.soundfont.as_deref(), Some("/a.sf2"));
        assert_eq!(s.volume, Some(80));
        assert!(Set::parse(r#"{"program":200}"#).is_err());
        assert!(Set::parse(r#"{"transpose":-49}"#).is_err());
        assert!(Set::parse(r#"{"volume":"loud"}"#).is_err());
        assert!(Set::parse(r#"{"other":1}"#).is_err());
        assert!(Set::parse(r#"{"soundfont":""}"#).is_err());
        assert!(Set::parse("nope").is_err());
    }

    #[test]
    fn round_trips() {
        let s = Set {
            transpose: Some(-14),
            ..Default::default()
        };
        assert_eq!(s.to_line(), "{\"transpose\":-14}\n");
        assert_eq!(Set::parse(s.to_line().trim()).unwrap(), s);
        let l = Live {
            keys: 9344,
            note: Some(67),
            ..Default::default()
        };
        let back: Live = serde_json::from_str(l.to_line().trim()).unwrap();
        assert_eq!(back, l);
    }
}
