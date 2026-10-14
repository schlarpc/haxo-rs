extern crate fluidsynth;

use fluidsynth::{audio, midi, settings, synth};
use log::{info, warn};
use std::thread;
use std::time::Duration;

use crate::alsa;

extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

const AUDIO_DEVICE_POLL_MS: u64 = 250;
const AUDIO_DEVICE_TIMEOUT_MS: u64 = 30_000;

pub fn try_init(
    sf2file: &str,
    banknum: i32,
) -> (synth::Synth, settings::Settings, audio::AudioDriver) {
    let mut settings = settings::Settings::new();
    // try to optimize for low latency
    if settings.setstr("audio.driver", "alsa") {
        warn!("Setting audio.driver in fluidsynth failed");
    }
    if settings.setint("audio.periods", 3) {
        warn!("Setting audio.periods in fluidsynth failed");
    }
    if settings.setint("audio.period-size", 64) {
        warn!("Setting audio.period-size in fluidsynth failed");
    }

    // The card can still be probing when we start at boot; wait for it
    // rather than exit and lean on the service manager's restarts.
    let mut alsa_dev = alsa::get_device();
    let mut waited_ms = 0;
    while alsa_dev.is_err() && waited_ms < AUDIO_DEVICE_TIMEOUT_MS {
        if waited_ms == 0 {
            info!("Waiting for the audio output device ...");
        }
        thread::sleep(Duration::from_millis(AUDIO_DEVICE_POLL_MS));
        waited_ms += AUDIO_DEVICE_POLL_MS;
        alsa_dev = alsa::get_device();
    }
    if alsa_dev.is_err() {
        panic!("Failed to find audio output device");
    }
    let alsa_dev = alsa_dev.unwrap();
    if settings.setstr("audio.alsa.device", &alsa_dev) {
        warn!("Failed to attach synth to headphone output {}", &alsa_dev);
    }
    if settings.setint("audio.realtime-prio", 99) {
        info!("Setting audio.realtime-prio in fluidsynth failed");
    }
    let mut syn = synth::Synth::new(&mut settings);

    const FSYNTH_GAIN: f32 = 1.0;
    syn.set_gain(FSYNTH_GAIN);
    if syn.get_gain() != FSYNTH_GAIN {
        warn!("Failed to set gain to {}", FSYNTH_GAIN);
    }

    let adriver = audio::AudioDriver::new(&mut settings, &mut syn);

    // Use FluidR3_GM.sf2 with multiple instruments for midi file
    let startup_sf2_filename = "/usr/share/sounds/sf2/FluidR3_GM.sf2";
    let sf2startup = syn.sfload(startup_sf2_filename, 1);

    if sf2startup == None {
        warn!("Failed to load sound font file {}", startup_sf2_filename);
    }

    // Enable polyphony for midi file
    if !syn.set_polyphony(16) {
        warn!("Failed to set polyphony to 16");
    }

    // Select bank 0
    syn.program_change(0, 0);

    // Play the midi file
    let player = midi::Player::new(&mut syn);
    player.add("/usr/share/haxo/Startup_Haxophone.mid");
    player.play();

    // Wait until midi file is finished
    while player.get_status() == midi::PlayerStatus::Playing {
        // wait ..
    }

    // Polyphony stays at 16 for the sax as well. The main loop already ends
    // each note before the next, and a preset can need several voices per
    // note (layers, stereo sample pairs): with polyphony 1 fluidsynth kept
    // one of them, sometimes a silent one, and logged "Failed to allocate a
    // synthesis process" for the rest.

    let sf2 = syn.sfload(sf2file, 1);

    if sf2 == None {
        warn!("Failed to load sound font file {}", sf2file);
    }

    // The startup soundfont is only for the jingle. Left loaded, it holds
    // its samples in memory, and programs missing from the user's soundfont
    // fall through to it. Unloaded after the user's soundfont is in, so
    // the channels always have presets to resolve to.
    if let Some(id) = sf2startup {
        if !syn.sfunload(id, true) {
            warn!("Failed to unload sound font file {}", startup_sf2_filename);
        }
        // Its samples were freed under the user's soundfont, where glibc
        // keeps them in the heap: hand those pages back (13 MB on a Zero 2 W).
        unsafe {
            malloc_trim(0);
        }
    }
    // select bank number
    syn.program_change(0, banknum);
    println!("Synth created");
    (syn, settings, adriver)
}

pub fn beep(synth: &synth::Synth, note: i32, vol: i32) {
    const MIDI_CC_VOLUME: i32 = 7;
    synth.noteon(0, note, vol);
    synth.cc(0, MIDI_CC_VOLUME, vol);
    thread::sleep(Duration::from_millis(100));
    synth.noteoff(0, note);
}
