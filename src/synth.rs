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
    // What the driver's callback renders with; boxed so its address holds
    // across closing and reopening.
    render: Box<Render>,
}

struct Render {
    synth: fluid_ptr,
    tap: Option<crate::tap::Producer>,
    // fluid_synth_process leaves reverb and chorus out of the dry output
    // unless given buffers for them (fluidsynth's own driver path mixes
    // them in). One stereo pair takes every effect channel, summed (the
    // documented wrap-around), and is added to the output.
    fx: [Vec<f32>; 2],
}

// Longer periods than this render without effects; ALSA's are 64 here.
const MAX_PERIOD: usize = 8192;

type AudioFunc = extern "C" fn(
    data: *mut std::ffi::c_void,
    len: i32,
    nfx: i32,
    fx: *mut *mut f32,
    nout: i32,
    out: *mut *mut f32,
) -> i32;

extern "C" {
    fn new_fluid_audio_driver2(settings: fluid_ptr, func: AudioFunc, data: fluid_ptr) -> fluid_ptr;
    fn delete_fluid_audio_driver(driver: fluid_ptr);
    fn fluid_synth_process(
        synth: fluid_ptr,
        len: i32,
        nfx: i32,
        fx: *mut *mut f32,
        nout: i32,
        out: *mut *mut f32,
    ) -> i32;
}

// fluidsynth's audio thread calls this for every period: render the synth
// into the driver's buffers (what the jack plays), then hand a copy to the
// tap. Nothing here allocates or waits.
extern "C" fn render(
    data: *mut std::ffi::c_void,
    len: i32,
    nfx: i32,
    fx: *mut *mut f32,
    nout: i32,
    out: *mut *mut f32,
) -> i32 {
    let r = unsafe { &mut *(data as *mut Render) };
    let n = len.max(0) as usize;
    unsafe {
        // fluid_synth_process mixes into the buffers: start from silence.
        for (bufs, count) in [(out, nout), (fx, nfx)] {
            for i in 0..count.max(0) as usize {
                let b = *bufs.add(i);
                if !b.is_null() {
                    std::ptr::write_bytes(b, 0, n);
                }
            }
        }
    }
    let result = if nfx == 0 && nout >= 2 && n <= MAX_PERIOD {
        let [fl, fr] = &mut r.fx;
        fl[..n].fill(0.0);
        fr[..n].fill(0.0);
        let mut ours = [fl.as_mut_ptr(), fr.as_mut_ptr()];
        let result = unsafe { fluid_synth_process(r.synth, len, 2, ours.as_mut_ptr(), nout, out) };
        let (left, right) = unsafe {
            (
                std::slice::from_raw_parts_mut(*out, n),
                std::slice::from_raw_parts_mut(*out.add(1), n),
            )
        };
        for i in 0..n {
            left[i] += fl[i];
            right[i] += fr[i];
        }
        result
    } else {
        unsafe { fluid_synth_process(r.synth, len, nfx, fx, nout, out) }
    };
    if let Some(tap) = r.tap.as_mut() {
        if nout >= 2 {
            let (left, right) = unsafe {
                (
                    std::slice::from_raw_parts(*out, n),
                    std::slice::from_raw_parts(*out.add(1), n),
                )
            };
            crate::tap::push(tap, left, right);
        }
    }
    result
}

impl Audio {
    /// With `tap`, a copy of what's played goes there (see tap.rs).
    pub fn new(tap: Option<crate::tap::Producer>) -> Self {
        Audio {
            driver: std::ptr::null_mut(),
            render: Box::new(Render {
                synth: std::ptr::null_mut(),
                tap,
                fx: [vec![0.0; MAX_PERIOD], vec![0.0; MAX_PERIOD]],
            }),
        }
    }

    pub fn is_open(&self) -> bool {
        !self.driver.is_null()
    }

    pub fn open(&mut self, settings: &settings::Settings, syn: &synth::Synth) -> bool {
        if !self.is_open() {
            self.render.synth = syn.to_raw() as fluid_ptr;
            let data = &mut *self.render as *mut Render as fluid_ptr;
            self.driver =
                unsafe { new_fluid_audio_driver2(settings.to_raw() as fluid_ptr, render, data) };
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
    volume: i32,
    tap: Option<crate::tap::Producer>,
) -> (synth::Synth, settings::Settings, Audio, Option<u32>) {
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

    set_volume(&syn, volume);

    let mut adriver = Audio::new(tap);
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
    (syn, settings, adriver, sf2)
}

// Swap the playing soundfont for `path` and select `program`: the new one
// loads before the old one goes, so the channel always has presets. This
// reads the whole file, so the caller stops for the load. Returns the new
// soundfont's id, or None (the old one stays) when it doesn't load.
pub fn switch_soundfont(
    syn: &synth::Synth,
    old: Option<u32>,
    path: &str,
    program: i32,
) -> Option<u32> {
    let Some(id) = syn.sfload(path, 1) else {
        warn!("Failed to load sound font file {}", path);
        return None;
    };
    if let Some(old) = old {
        if !syn.sfunload(old, true) {
            warn!("Failed to unload the previous sound font");
        }
        unsafe {
            malloc_trim(0);
        }
    }
    syn.program_change(0, program);
    Some(id)
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

    extern "C" {
        fn fluid_synth_write_float(
            synth: fluid_ptr,
            len: i32,
            lout: *mut f32,
            loff: i32,
            lincr: i32,
            rout: *mut f32,
            roff: i32,
            rincr: i32,
        ) -> i32;
    }

    // A synth playing a note through a reverberant preset.
    fn playing(sf2: &str) -> synth::Synth {
        let mut settings = settings::Settings::new();
        let syn = synth::Synth::new(&mut settings);
        std::mem::forget(settings);
        syn.sfload(sf2, 1).expect("soundfont loads");
        syn.program_change(0, 66);
        syn.cc(0, 91, 127); // reverb send, full
        syn.noteon(0, 60, 127);
        syn
    }

    // The tap's render callback must sound like fluidsynth's own driver
    // path, effects included: fluid_synth_process leaves them out unless
    // given buffers for them.
    #[test]
    fn callback_renders_like_the_driver() {
        let Ok(sf2) = std::env::var("HAXO_TEST_SF2") else {
            return;
        };
        const N: usize = 64;
        const PERIODS: usize = 200;
        let reference = playing(&sf2);
        let ours = playing(&sf2);
        let mut audio = Audio::new(None);
        audio.render.synth = ours.to_raw() as fluid_ptr;
        let data = &mut *audio.render as *mut Render as *mut std::ffi::c_void;
        let (mut diff, mut energy, mut wet_diff) = (0f64, 0f64, 0f64);
        let bare = playing(&sf2);
        for _ in 0..PERIODS {
            let (mut rl, mut rr) = ([0f32; N], [0f32; N]);
            unsafe {
                fluid_synth_write_float(
                    reference.to_raw() as fluid_ptr,
                    N as i32,
                    rl.as_mut_ptr(),
                    0,
                    1,
                    rr.as_mut_ptr(),
                    0,
                    1,
                )
            };
            let (mut l, mut r) = ([0f32; N], [0f32; N]);
            let mut out = [l.as_mut_ptr(), r.as_mut_ptr()];
            render(data, N as i32, 0, std::ptr::null_mut(), 2, out.as_mut_ptr());
            // And without effects, to show the test can tell.
            let (mut bl, mut br) = ([0f32; N], [0f32; N]);
            let mut bout = [bl.as_mut_ptr(), br.as_mut_ptr()];
            unsafe {
                fluid_synth_process(
                    bare.to_raw() as fluid_ptr,
                    N as i32,
                    0,
                    std::ptr::null_mut(),
                    2,
                    bout.as_mut_ptr(),
                )
            };
            for i in 0..N {
                diff += ((l[i] - rl[i]) as f64).powi(2) + ((r[i] - rr[i]) as f64).powi(2);
                wet_diff += ((bl[i] - rl[i]) as f64).powi(2) + ((br[i] - rr[i]) as f64).powi(2);
                energy += (rl[i] as f64).powi(2) + (rr[i] as f64).powi(2);
            }
        }
        println!(
            "energy {:.3e}, ours vs driver {:.3e}, without effects {:.3e}",
            energy, diff, wet_diff
        );
        assert!(energy > 0.0);
        assert!(
            diff < energy * 1e-6,
            "callback output differs from the driver's"
        );
        assert!(
            wet_diff > energy * 1e-4,
            "the test can't tell effects from none"
        );
    }

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

// Volume 0-100 as the synth's gain, 100 being a gain of 1.0.
pub fn set_volume(syn: &synth::Synth, volume: i32) {
    let gain = volume.clamp(0, 100) as f32 / 100.0;
    syn.set_gain(gain);
    if syn.get_gain() != gain {
        warn!("Failed to set gain to {}", gain);
    }
}

/// The rate the synth renders at.
pub fn sample_rate(settings: &settings::Settings) -> u32 {
    settings.getnum("synth.sample-rate").unwrap_or(44100.0) as u32
}
