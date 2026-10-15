use log::{debug, info, log_enabled, warn, Level};

#[cfg(feature = "instrumentation")]
use rppal::gpio::Gpio;

use std::cmp::max;
use std::error::Error;
use std::process::Command;
use std::time::{Duration, Instant};

use structopt::StructOpt;

mod alsa;
#[cfg(feature = "ble")]
mod ble;
mod commands;
mod control;
mod cues;
mod idle;
mod keyscan;
#[cfg(feature = "midi")]
mod midi;
mod midinotes;
mod notemap;
mod pressure;
mod synth;
mod tap;
mod ticks;
mod transpose;

#[derive(Debug, StructOpt)]
#[structopt(name = "haxo", about = "Make music on a haxophone", version = env!("VERGEN_GIT_DESCRIBE"), settings = &[structopt::clap::AppSettings::AllowNegativeNumbers])]
struct Opt {
    #[structopt(short, long)]
    record: bool,
    #[structopt(short, long, default_value = "/usr/share/sounds/sf2/FluidR3_GM.sf2")]
    sf2_file: String,
    #[structopt(short, long, default_value = "67")]
    prog_number: i32,
    #[structopt(short, long, default_value = "./notemap.json")]
    notemap_file: String,
    #[structopt(short, long, default_value = "-14")]
    transpose: i32,
    /// Soundfont for the startup jingle; unloaded once it has played
    #[structopt(long, default_value = "/usr/share/sounds/sf2/FluidR3_GM.sf2")]
    startup_sf2_file: String,
    /// ALSA rawmidi device for MIDI out; when it can't be opened, the last
    /// sequencer port
    #[cfg(feature = "midi")]
    #[structopt(long, default_value = "hw:CARD=haxophone")]
    midi_out: String,
    /// Also send MIDI over Bluetooth LE, advertised under this name
    #[cfg(feature = "ble")]
    #[structopt(long)]
    ble_midi: Option<String>,
    /// The rate the synth renders and the card plays at, in Hz. 48000 is
    /// what Bluetooth and USB audio run at, and what ALSA's dmix defaults to.
    #[structopt(long, default_value = "48000")]
    sample_rate: u32,
    /// Volume, 0-100 (the synth's gain; 100 is a gain of 1)
    #[structopt(long, default_value = "100")]
    volume: i32,
    /// Unix socket for live state and settings (JSON lines); off when unset
    #[structopt(long)]
    control_socket: Option<String>,
    /// Unix socket for a copy of the sound (raw PCM, see haxo-control);
    /// off when unset
    #[structopt(long)]
    audio_socket: Option<String>,
    /// Seconds without breath or key changes before the audio output is
    /// closed (shutting the amp down) until the next one; 0 never closes it
    #[structopt(long, default_value = "0")]
    idle_secs: u64,
    /// Startup jingle (a MIDI file); empty to start without one
    #[structopt(long, default_value = "/usr/share/haxo/Startup_Haxophone.mid")]
    startup_midi_file: String,
}

#[derive(PartialEq)]
enum Mode {
    Play,
    Control,
    Transpose,
}

#[allow(dead_code)]
fn shutdown() {
    debug!("Bye...");
    Command::new("/usr/bin/sudo")
        .arg("/usr/sbin/halt")
        .status()
        .expect("failed to halt system");
}

const TICK_USECS: u32 = 2_000;
// The tick while idle: enough to notice breath or keys.
const IDLE_TICK_USECS: u64 = 20_000;

#[cfg(feature = "instrumentation")]
const GPIO_UART_RXD: u8 = 15;
#[cfg(feature = "instrumentation")]
const GPIO_UART_TXD: u8 = 14;

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let opt = Opt::from_args();
    debug!("{:?}", opt);

    let (tap_in, tap_out) = match opt.audio_socket {
        Some(_) => {
            let (p, c) = tap::ring(opt.sample_rate);
            (Some(p), Some(c))
        }
        None => (None, None),
    };
    let (synth, settings, mut audio, mut soundfont_id) = synth::try_init(
        &opt.sf2_file,
        opt.prog_number,
        &opt.startup_sf2_file,
        &opt.startup_midi_file,
        opt.volume,
        opt.sample_rate,
        tap_in,
    );
    if let (Some(path), Some(ring)) = (opt.audio_socket.as_deref(), tap_out) {
        let format = haxo_control::AudioFormat::s16le(synth::sample_rate(&settings), 1);
        if let Err(e) = tap::serve(path, ring, format) {
            warn!("No audio tap at {}: {}", path, e);
        }
    }
    let mut control = opt.control_socket.as_deref().and_then(|path| {
        control::Control::start(path)
            .map_err(|e| warn!("No control socket at {}: {}", path, e))
            .ok()
    });
    let mut live = control::Live {
        program: opt.prog_number,
        transpose: opt.transpose,
        volume: opt.volume,
        soundfont: opt.sf2_file.clone(),
        ..Default::default()
    };
    #[cfg(feature = "midi")]
    let mut midi_out = midi::MidiOut::new(&opt.midi_out, {
        #[cfg(feature = "ble")]
        let ble = opt.ble_midi.as_deref();
        #[cfg(not(feature = "ble"))]
        let ble = None;
        ble
    });

    // Use UART RXD pin to monitor timing of periodic task.  This is easily
    // accessible on the haxophone HAT when the console is disabled.
    #[cfg(feature = "instrumentation")]
    let mut busy_pin = Gpio::new()?.get(GPIO_UART_RXD)?.into_output();
    #[cfg(feature = "instrumentation")]
    let mut noteon_pin = Gpio::new()?.get(GPIO_UART_TXD)?.into_output();

    println!(
        "Starting haxophone (version {})...",
        env!("VERGEN_GIT_DESCRIBE")
    );

    keyscan::init_io().expect("Failed to initialize scan GPIO");
    let mut sensor = pressure::Pressure::init().expect("Failed to initialize pressure sensor");

    let mut notemap = notemap::NoteMap::generate(&opt.notemap_file, opt.transpose);
    if opt.record {
        notemap.start_recording();
    }

    let mut last_note = 0;
    let mut mode = Mode::Play;
    let cues = cues::Cues::new(&synth);
    let mut cmd = commands::Command::new(&synth, &cues, opt.prog_number);

    const TRANSPOSE_COUNTDOWN_MS: u32 = 200u32;
    const TRANSPOSE_COUNTDOWN_TICKS: u32 = TRANSPOSE_COUNTDOWN_MS * 1000 / TICK_USECS;
    let mut transpose = transpose::Transpose::new(&cues, TRANSPOSE_COUNTDOWN_TICKS);

    const NEG_PRESS_COUNTDOWN_MS: u32 = 500u32;
    const NEG_PRESS_INIT_VAL: u32 = NEG_PRESS_COUNTDOWN_MS * 1000 / TICK_USECS;
    let mut neg_pressure_countdown: u32 = NEG_PRESS_INIT_VAL;
    let mut last_vol: i32 = 0;
    // After every thread is started: new threads would inherit the policy.
    ticks::make_realtime();
    let tick_period = Duration::from_micros(TICK_USECS as u64);
    let mut tick = ticks::Ticker::new(tick_period);
    let mut tick_stats = ticks::TickStats::new(tick_period);
    let idle_after = Some(Duration::from_secs(opt.idle_secs)).filter(|d| !d.is_zero());
    let mut idle = idle::Idle::new(idle_after, Instant::now());
    loop {
        tick_stats.idle();
        tick.wait();
        tick_stats.wake();
        cues.poll();
        #[cfg(feature = "instrumentation")]
        busy_pin.set_high();

        let keys = keyscan::scan()?;
        let pressure = sensor.read()?;
        let vol = max(0, pressure);

        match idle.update(Instant::now(), keys, pressure) {
            idle::Change::Sleep => {
                audio.close();
                tick.set_period(Duration::from_micros(IDLE_TICK_USECS));
                tick_stats.set_period(Duration::from_micros(IDLE_TICK_USECS));
                info!("Idle: audio closed");
            }
            idle::Change::Wake => {
                let started = Instant::now();
                audio.open(&settings, &synth);
                tick.set_period(tick_period);
                tick_stats.set_period(tick_period);
                info!(
                    "Awake: audio reopened in {} ms",
                    started.elapsed().as_millis()
                );
            }
            idle::Change::None => (),
        }

        const MIDI_CC_VOLUME: i32 = 7;
        if last_vol != vol {
            synth.cc(0, MIDI_CC_VOLUME, vol);
            #[cfg(feature = "midi")]
            midi_out.breath(vol);
            last_vol = vol;
        }
        #[cfg(feature = "midi")]
        midi_out.poll();

        if let Some(control) = control.as_mut() {
            for set in control.commands() {
                if let Some(path) = set.soundfont {
                    let program = set.program.unwrap_or(cmd.program());
                    info!("Loading sound font {}", path);
                    if let Some(id) = synth::switch_soundfont(&synth, soundfont_id, &path, program)
                    {
                        soundfont_id = Some(id);
                        live.soundfont = path;
                        cmd.reload_programs();
                        cmd.set_program(program);
                    }
                } else if let Some(p) = set.program {
                    cmd.set_program(p);
                }
                if let Some(t) = set.transpose {
                    notemap.transpose = t;
                    info!("Set transpose to {}", t);
                }
                if let Some(v) = set.volume {
                    synth::set_volume(&synth, v);
                    live.volume = v;
                }
            }
            live.keys = keys;
            live.breath = vol;
            live.note = if last_note > 0 { Some(last_note) } else { None };
            live.program = cmd.program();
            live.transpose = notemap.transpose;
            control.publish(&live);
        }

        if notemap.is_recording() {
            notemap.record(keys, pressure);
        }

        if mode == Mode::Control {
            cmd.process(keys);
        } else if mode == Mode::Transpose {
            transpose.process(keys, vol, &mut notemap);
        }

        if mode != Mode::Play {
            // All three left hand palm keys pressed at once
            if keys == 0x124 {
                cues.beep(70, 50, Duration::ZERO);
                mode = Mode::Play;
                info!("Return to Play Mode");
            }
            continue;
        }

        if let Some(note) = notemap.get(&keys) {
            if last_note != note {
                if log_enabled!(Level::Debug) {
                    debug!(
                        "Note: {} Pressure: {} Key {:032b}: {}",
                        notemap.get_name(&note).unwrap_or("Unknown?"),
                        pressure,
                        keys,
                        keys
                    );
                };
                if vol > 0 {
                    if last_note > 0 {
                        synth.cc(0, MIDI_CC_VOLUME, 0);
                        synth.noteoff(0, last_note);
                        #[cfg(feature = "midi")]
                        midi_out.noteoff(last_note);
                        #[cfg(feature = "instrumentation")]
                        noteon_pin.set_low();
                        synth.cc(0, MIDI_CC_VOLUME, vol);
                    }
                    synth.noteon(0, note, 127);
                    debug!("active voices: {}", synth.get_active_voice_count());
                    #[cfg(feature = "midi")]
                    midi_out.noteon(note, 127);
                    #[cfg(feature = "instrumentation")]
                    noteon_pin.set_high();
                    last_note = note;
                    debug!("last_note changed to {}", last_note);
                }
            }
            if vol <= 0 && last_note > 0 {
                synth.noteoff(0, last_note);
                #[cfg(feature = "midi")]
                midi_out.noteoff(last_note);
                #[cfg(feature = "instrumentation")]
                noteon_pin.set_low();
                last_note = 0;
            }

            // Negative pressure needs to hold for a minimum duration to trigger a mode change
            if pressure < -10 {
                neg_pressure_countdown = neg_pressure_countdown.wrapping_sub(1);
            } else {
                neg_pressure_countdown = NEG_PRESS_INIT_VAL;
            }

            // Enter Control Mode
            if neg_pressure_countdown == 0 {
                match notemap.get_name(&note) {
                    Some("Low Bb") => {
                        mode = Mode::Control;
                        cues.beep(71, 50, Duration::ZERO);
                        info!("Enter Control Mode");
                    }
                    Some("Low B") => {
                        mode = Mode::Transpose;
                        cues.beep(71, 50, Duration::ZERO);
                        cues.beep(75, 50, Duration::from_millis(120));
                        info!("Enter Transpose Mode");
                    }
                    _ => {}
                }
            }
        } else {
            if log_enabled!(Level::Debug) {
                keyscan::debug_print(keys);
            }
        }
        #[cfg(feature = "instrumentation")]
        busy_pin.set_low();
    }
}
