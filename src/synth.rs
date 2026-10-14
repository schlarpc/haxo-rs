extern crate fluidsynth;

use fluidsynth::{midi, settings, synth};
use log::{info, warn};
use std::thread;
use std::time::Duration;

use crate::alsa;

extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

// The audio driver: fluidsynth's thread that renders the synth into the
// ALSA device. Closing it closes the PCM stream, and the MAX98357A's codec
// driver then pulls SD_MODE low, shutting the amp down. The binding's
// AudioDriver needs the synth mutably, which the main loop can't offer.
pub struct Audio {
    driver: *mut std::ffi::c_void,
}

extern "C" {
    fn new_fluid_audio_driver(settings: fluid_ptr, synth: fluid_ptr) -> fluid_ptr;
    fn delete_fluid_audio_driver(driver: fluid_ptr);
}

impl Audio {
    pub fn new() -> Self {
        Audio {
            driver: std::ptr::null_mut(),
        }
    }

    pub fn is_open(&self) -> bool {
        !self.driver.is_null()
    }

    pub fn open(&mut self, settings: &settings::Settings, syn: &synth::Synth) -> bool {
        if !self.is_open() {
            self.driver = unsafe {
                new_fluid_audio_driver(settings.to_raw() as fluid_ptr, syn.to_raw() as fluid_ptr)
            };
            if !self.is_open() {
                warn!("Failed to open the audio driver");
            }
        }
        self.is_open()
    }

    pub fn close(&mut self) {
        if self.is_open() {
            unsafe { delete_fluid_audio_driver(self.driver) };
            self.driver = std::ptr::null_mut();
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.close();
    }
}

// fluidsynth 2's soundfont API; the binding only has fluidsynth 1's.
#[allow(non_camel_case_types)]
type fluid_ptr = *mut std::ffi::c_void;
extern "C" {
    fn fluid_synth_sfcount(synth: fluid_ptr) -> i32;
    fn fluid_synth_get_sfont(synth: fluid_ptr, num: u32) -> fluid_ptr;
    fn fluid_sfont_iteration_start(sfont: fluid_ptr);
    fn fluid_sfont_iteration_next(sfont: fluid_ptr) -> fluid_ptr;
    fn fluid_preset_get_banknum(preset: fluid_ptr) -> i32;
    fn fluid_preset_get_num(preset: fluid_ptr) -> i32;
}

// The program numbers in `bank` across the loaded soundfonts, sorted.
pub fn programs(syn: &synth::Synth, bank: i32) -> Vec<i32> {
    let raw = syn.to_raw() as fluid_ptr;
    let mut programs = Vec::new();
    unsafe {
        for num in 0..fluid_synth_sfcount(raw) {
            let sfont = fluid_synth_get_sfont(raw, num as u32);
            if sfont.is_null() {
                continue;
            }
            fluid_sfont_iteration_start(sfont);
            loop {
                let preset = fluid_sfont_iteration_next(sfont);
                if preset.is_null() {
                    break;
                }
                if fluid_preset_get_banknum(preset) == bank {
                    programs.push(fluid_preset_get_num(preset));
                }
            }
        }
    }
    programs.sort();
    programs.dedup();
    programs
}

const AUDIO_DEVICE_POLL_MS: u64 = 250;
const AUDIO_DEVICE_TIMEOUT_MS: u64 = 30_000;

pub fn try_init(
    sf2file: &str,
    banknum: i32,
    startup_sf2_file: &str,
    startup_midi_file: &str,
) -> (synth::Synth, settings::Settings, Audio) {
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

    let mut adriver = Audio::new();
    adriver.open(&settings, &syn);

    // Polyphony 16 for the jingle and for the sax alike. The main loop ends
    // each note before the next, and a preset can need several voices per
    // note (layers, stereo sample pairs): with polyphony 1 fluidsynth kept
    // one of them, sometimes a silent one, and logged "Failed to allocate a
    // synthesis process" for the rest.
    if !syn.set_polyphony(16) {
        warn!("Failed to set polyphony to 16");
    }

    let sf2startup = if startup_midi_file.is_empty() {
        None
    } else {
        play_jingle(&mut syn, startup_sf2_file, startup_midi_file)
    };

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
            warn!("Failed to unload sound font file {}", startup_sf2_file);
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

// Play the startup jingle with its own soundfont (a General MIDI one: the
// jingle uses several instruments), and return that soundfont's id.
fn play_jingle(syn: &mut synth::Synth, sf2file: &str, midifile: &str) -> Option<u32> {
    let id = syn.sfload(sf2file, 1);
    if id == None {
        warn!("Failed to load sound font file {}", sf2file);
    }
    syn.program_change(0, 0);

    let player = midi::Player::new(syn);
    player.add(midifile);
    player.play();
    while player.get_status() == midi::PlayerStatus::Playing {
        thread::sleep(Duration::from_millis(10));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    // Needs a soundfont in HAXO_TEST_SF2; passes vacuously without one.
    #[test]
    fn lists_the_programs_of_a_bank() {
        let Ok(sf2) = std::env::var("HAXO_TEST_SF2") else {
            return;
        };
        let mut settings = settings::Settings::new();
        let syn = synth::Synth::new(&mut settings);
        // Deleting the synth after its settings hangs.
        std::mem::forget(settings);
        assert!(programs(&syn, 0).is_empty());
        syn.sfload(&sf2, 1).expect("soundfont loads");
        let progs = programs(&syn, 0);
        println!(
            "bank 0: {} programs, bank 128: {}",
            progs.len(),
            programs(&syn, 128).len()
        );
        assert!(!progs.is_empty());
        assert!(progs.windows(2).all(|w| w[0] < w[1]));
        assert!(progs.iter().all(|p| (0..128).contains(p)));
    }
}
