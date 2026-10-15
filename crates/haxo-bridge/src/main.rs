//! haxo-bridge: plays haxo001's audio tap (--audio-socket) into an ALSA
//! device that runs on its own clock: the USB audio gadget (the host's
//! clock) or PipeWire's ALSA plugin feeding a Bluetooth speaker (the
//! Bluetooth link's). Not the jack: that's haxo001's own output.
//!
//! Two clocks never agree exactly, so a fixed buffer between them drains or
//! fills by a few dozen samples a second. The bridge keeps a small FIFO and
//! steers what's buffered (FIFO plus the device's own queue) to a target by
//! nudging a `Slip` resampler's ratio: it plays samples through untouched
//! and now and then slips one frame, crossfaded, which costs almost no CPU
//! and adds no delay. When haxo001 stalls or restarts, the device gets
//! silence rather than underrunning.
//!
//! A separate process at normal priority: it reads the tap like any
//! listener, so nothing it does can delay the in-ear path.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use alsa::pcm::{Access, Format, HwParams, State, PCM};
use alsa::{Direction, ValueOr};
use clap::Parser;
use haxo_control::AudioFormat;
use log::{info, warn};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Adjustable, FixedAsync, Resampler, Slip};

#[derive(Parser, Debug)]
#[command(version, about = "Play haxo001's audio tap into an ALSA device")]
struct Args {
    /// haxo001's audio tap.
    #[arg(long, default_value = "/run/haxo/audio.sock")]
    tap: PathBuf,
    /// ALSA device to play into.
    #[arg(long)]
    device: String,
    /// Channels to open it with; the mono tap goes to each.
    #[arg(long, default_value_t = 2)]
    channels: u32,
    /// The tap's rate, which the device is opened at.
    #[arg(long, default_value_t = 48000)]
    rate: u32,
    /// Frames per period (each write).
    #[arg(long, default_value_t = 256)]
    period: usize,
    /// Periods in the device's buffer.
    #[arg(long, default_value_t = 6)]
    periods: usize,
    /// What to keep buffered between the tap and the speaker, in ms
    /// (this FIFO plus the device's queue).
    #[arg(long, default_value_t = 40.0)]
    target_ms: f64,
}

// Ratio change per second of buffering error: 10 ms off target plays 0.2%
// faster or slower. Crystal drift is tens of ppm, so it settles a fraction
// of a millisecond from the target.
const GAIN: f64 = 0.2;
const MAX_CORRECTION: f64 = 0.005;
// Per loop pass (a period, ~5 ms): about half a second.
const SMOOTHING: f64 = 0.01;
// More than this over the target is dropped at once; the ratio only
// handles drift.
const SNAP: f64 = 0.02;
// Past this the FIFO drops back to the target outright.
const MAX_FIFO_MS: f64 = 150.0;
const REPORT: Duration = Duration::from_secs(10);

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    // Retries every second; each new state is logged once, not every try.
    // The USB gadget refuses audio (EIO) whenever the host isn't recording
    // from it, which is the usual state, not a fault.
    let mut last = String::new();
    loop {
        if let Err(e) = run(&args) {
            if e != last {
                warn!("{}", e);
                last = e;
            }
        } else {
            last.clear();
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn connect(path: &PathBuf) -> std::io::Result<(BufReader<UnixStream>, AudioFormat)> {
    let stream = UnixStream::connect(path)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let format: AudioFormat =
        serde_json::from_str(&line).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    if format.channels != 1 || format.format != "s16le" {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("can't play {:?}", format),
        ));
    }
    reader.get_ref().set_nonblocking(true)?;
    Ok((reader, format))
}

fn open(args: &Args, rate: u32) -> alsa::Result<PCM> {
    let pcm = PCM::new(&args.device, Direction::Playback, false)?;
    {
        let hw = HwParams::any(&pcm)?;
        hw.set_access(Access::RWInterleaved)?;
        hw.set_format(Format::s16())?;
        hw.set_channels(args.channels)?;
        hw.set_rate(rate, ValueOr::Nearest)?;
        hw.set_period_size_near(args.period as alsa::pcm::Frames, ValueOr::Nearest)?;
        hw.set_buffer_size_near((args.period * args.periods) as alsa::pcm::Frames)?;
        pcm.hw_params(&hw)?;
    }
    Ok(pcm)
}

struct Stats {
    since: Instant,
    underruns: u32,
    xruns: u32,
    dropped_ms: f64,
    buffered_ms: (f64, f64),
    ppm: (f64, f64),
}

impl Stats {
    fn new() -> Self {
        Stats {
            since: Instant::now(),
            underruns: 0,
            xruns: 0,
            dropped_ms: 0.0,
            buffered_ms: (f64::MAX, 0.0),
            ppm: (f64::MAX, f64::MIN),
        }
    }
    fn note(&mut self, buffered_ms: f64, ppm: f64) {
        self.buffered_ms = (
            self.buffered_ms.0.min(buffered_ms),
            self.buffered_ms.1.max(buffered_ms),
        );
        self.ppm = (self.ppm.0.min(ppm), self.ppm.1.max(ppm));
    }
    fn report(&mut self) {
        if self.since.elapsed() < REPORT {
            return;
        }
        info!(
            "buffered {:.1}-{:.1} ms, ratio {:+.0}..{:+.0} ppm, {} underruns (silence), {} xruns, {:.0} ms dropped",
            self.buffered_ms.0, self.buffered_ms.1, self.ppm.0, self.ppm.1,
            self.underruns, self.xruns, self.dropped_ms
        );
        *self = Stats::new();
    }
}

fn run(args: &Args) -> Result<(), String> {
    // The device first, kept open while it isn't taking audio: the USB
    // gadget while no host records, PipeWire while no speaker is
    // connected. The tap needn't be bothered (a listener coming and going
    // every second) until there's somewhere to play.
    let pcm = open(args, args.rate).map_err(|e| format!("ALSA {}: {}", args.device, e))?;
    let (buffer, period) = pcm
        .get_params()
        .map(|(b, p)| (b as usize, p as usize))
        .unwrap_or((args.period * args.periods, args.period));
    let period = period.max(4);
    let channels = args.channels as usize;
    let mut announced = false;
    let io = pcm.io_i16().map_err(|e| e.to_string())?;
    let mut waiting = false;
    loop {
        match taking(&pcm, &io, period, channels, buffer, args.rate) {
            Ok(true) => break,
            Ok(false) => {
                if !waiting {
                    info!(
                        "{} isn't taking audio (nobody recording from it?); waiting",
                        args.device
                    );
                    waiting = true;
                }
                thread::sleep(Duration::from_secs(1));
            }
            Err(e) => return Err(format!("ALSA {}: {}", args.device, e)),
        }
    }
    let (mut tap, format) =
        connect(&args.tap).map_err(|e| format!("tap {}: {}", args.tap.display(), e))?;
    if format.rate != args.rate {
        return Err(format!(
            "the tap is {} Hz, the device was opened at {} Hz (--rate)",
            format.rate, args.rate
        ));
    }
    let rate = format.rate as f64;
    let mut slip = Slip::<f32>::new(period, 1, FixedAsync::Output).map_err(|e| e.to_string())?;

    let target = args.target_ms / 1000.0 * rate;
    let max_fifo = (MAX_FIFO_MS / 1000.0 * rate) as usize;
    let mut fifo: VecDeque<f32> = VecDeque::with_capacity(max_fifo * 2);
    let mut buf = vec![0u8; 16384];
    let mut carry: Option<u8> = None;
    let mut input: Vec<f32> = Vec::new();
    let mut output = vec![0f32; period];
    let mut frames = vec![0i16; period * channels];
    let silence = vec![0i16; period * channels];
    let mut started = false;
    let mut smoothed = 0.0f64;
    let mut stats = Stats::new();

    loop {
        // Everything the tap has sent since last time.
        loop {
            match tap.read(&mut buf) {
                Ok(0) => return Err("tap closed".into()),
                Ok(n) => {
                    let mut bytes = &buf[..n];
                    if let Some(lo) = carry.take() {
                        fifo.push_back(i16::from_le_bytes([lo, bytes[0]]) as f32 / 32768.0);
                        bytes = &bytes[1..];
                    }
                    let mut it = bytes.chunks_exact(2);
                    for s in &mut it {
                        fifo.push_back(i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0);
                    }
                    carry = it.remainder().first().copied();
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(format!("tap: {}", e)),
            }
        }
        if fifo.len() > max_fifo {
            let drop = fifo.len() - target as usize;
            fifo.drain(..drop);
            stats.dropped_ms += drop as f64 / rate * 1000.0;
        }
        // Start once the whole target is there: starting short leaves a thin
        // margin while the ratio builds it up, and one late burst from the
        // tap then runs the device dry.
        if !started {
            if (fifo.len() as f64) < target {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            started = true;
        }

        // What waits in this bridge's own ALSA buffer. Not snd_pcm_delay:
        // through PipeWire that includes everything downstream (a
        // Bluetooth codec and link, 100 ms and more), which isn't ours to
        // steer and made the bridge drop all its input as "too much".
        let queued = buffer.saturating_sub(pcm.avail_update().unwrap_or(0).max(0) as usize) as f64;
        // The ratio first: it decides how much input the next chunk takes.
        // The error is smoothed over about half a second: the tap delivers
        // in ~5 ms bursts, and following each would slip far more often
        // than the clocks' real difference needs.
        let mut err = (fifo.len() as f64 + queued - target) / rate;
        if err > SNAP {
            // Far over (starting, or after a stall): drop the excess now
            // instead of slipping it away over many seconds.
            let excess = ((err * rate) as usize).min(fifo.len());
            fifo.drain(..excess);
            stats.dropped_ms += excess as f64 / rate * 1000.0;
            err = (fifo.len() as f64 + queued - target) / rate;
            smoothed = err;
        }
        smoothed += SMOOTHING * (err - smoothed);
        let correction = (-GAIN * smoothed).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        let _ = slip.set_resample_ratio_relative(1.0 + correction, true);
        let need = slip.input_frames_next();
        let write = if fifo.len() >= need {
            stats.note(
                (fifo.len() as f64 + queued) / rate * 1000.0,
                correction * 1e6,
            );

            input.clear();
            input.extend(fifo.drain(..need));
            let inp = InterleavedSlice::new(&input, 1, need).map_err(|e| e.to_string())?;
            let mut out =
                InterleavedSlice::new_mut(&mut output, 1, period).map_err(|e| e.to_string())?;
            let (_, produced) = slip
                .process_into_buffer(&inp, &mut out, None)
                .map_err(|e| e.to_string())?;
            for (i, s) in output[..produced].iter().enumerate() {
                let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                for c in 0..channels {
                    frames[i * channels + c] = v;
                }
            }
            Some(&frames[..produced * channels])
        } else if queued < period as f64 {
            // The tap is late and the device about to run dry: silence.
            stats.underruns += 1;
            Some(&silence[..])
        } else {
            thread::sleep(Duration::from_millis(1));
            None
        };
        if let Some(w) = write {
            match io.writei(w) {
                Ok(_) if !announced => {
                    info!(
                        "Playing the tap ({} Hz) into {} ({} ch, {} frame periods), keeping {} ms buffered",
                        format.rate, args.device, channels, period, args.target_ms
                    );
                    announced = true;
                }
                Ok(_) => {}
                Err(e) if e.errno() as i32 == libc_eio() => {
                    return Err(if announced {
                        format!(
                            "{} stopped taking audio (nobody recording?); waiting",
                            args.device
                        )
                    } else {
                        format!(
                            "{} isn't taking audio (nobody recording from it?); waiting",
                            args.device
                        )
                    });
                }
                Err(e) => {
                    stats.xruns += 1;
                    pcm.try_recover(e, true)
                        .map_err(|e| format!("ALSA: {}", e))?;
                }
            }
        }
        stats.report();
    }
}

/// Whether the device consumes audio: a period of silence, started, and a
/// look a few periods later at whether any of it was played. The idle USB
/// gadget takes the write and the start, and then never moves (a write
/// that waits on it fails, EIO); PipeWire with no speaker blocks here until
/// one connects. Leaves the device prepared for the real thing.
fn taking(
    pcm: &PCM,
    io: &alsa::pcm::IO<i16>,
    period: usize,
    channels: usize,
    buffer: usize,
    rate: u32,
) -> alsa::Result<bool> {
    pcm.drop()?;
    pcm.prepare()?;
    let started = io
        .writei(&vec![0i16; period * channels])
        .and_then(|_| match pcm.state() {
            State::Running => Ok(()),
            _ => pcm.start(),
        });
    let moved = match started {
        Err(e) if e.errno() as i32 == libc_eio() => false,
        Err(e) => return Err(e),
        Ok(()) => {
            thread::sleep(Duration::from_secs_f64(3.0 * period as f64 / rate as f64));
            match pcm.avail_update() {
                Ok(avail) => avail as usize > buffer - period,
                // Played it all and ran dry: it moves.
                Err(e) if e.errno() as i32 == libc_epipe() => true,
                Err(e) if e.errno() as i32 == libc_eio() => false,
                Err(e) => return Err(e),
            }
        }
    };
    pcm.drop()?;
    pcm.prepare()?;
    Ok(moved)
}

// EIO and EPIPE, without a libc dependency for two numbers.
fn libc_eio() -> i32 {
    5
}
fn libc_epipe() -> i32 {
    32
}
