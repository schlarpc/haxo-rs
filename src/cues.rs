// Short feedback notes (mode changes, program and transpose changes),
// scheduled instead of slept through: the main loop calls poll() every tick
// and keeps scanning while a cue plays.
use std::cell::RefCell;
use std::time::{Duration, Instant};

use fluidsynth::synth::Synth;

const BEEP_LENGTH: Duration = Duration::from_millis(100);
const MIDI_CC_VOLUME: i32 = 7;

enum Event {
    On(i32, i32),
    Off(i32),
}

pub struct Cues<'a> {
    synth: &'a Synth,
    // Events not played yet, in the order they were scheduled.
    pending: RefCell<Vec<(Instant, Event)>>,
}

impl<'a> Cues<'a> {
    pub fn new(synth: &'a Synth) -> Self {
        Cues {
            synth,
            pending: RefCell::new(Vec::new()),
        }
    }

    // Play `note` at `vol` for BEEP_LENGTH, starting `delay` from now.
    pub fn beep(&self, note: i32, vol: i32, delay: Duration) {
        let start = Instant::now() + delay;
        let mut pending = self.pending.borrow_mut();
        pending.push((start, Event::On(note, vol)));
        pending.push((start + BEEP_LENGTH, Event::Off(note)));
        drop(pending);
        self.poll();
    }

    // Play every event that is due.
    pub fn poll(&self) {
        let mut pending = self.pending.borrow_mut();
        if pending.is_empty() {
            return;
        }
        let now = Instant::now();
        // Stable, so events at the same instant keep their order.
        pending.sort_by_key(|(at, _)| *at);
        let due = pending.iter().take_while(|(at, _)| *at <= now).count();
        for (_, event) in pending.drain(..due) {
            match event {
                Event::On(note, vol) => {
                    self.synth.noteon(0, note, vol);
                    self.synth.cc(0, MIDI_CC_VOLUME, vol);
                }
                Event::Off(note) => {
                    self.synth.noteoff(0, note);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluidsynth::settings::Settings;
    use std::thread::sleep;

    // Needs no audio hardware: a synth without an audio driver, and a
    // soundfont from HAXO_TEST_SF2.
    fn synth() -> Option<Synth> {
        let sf2 = std::env::var("HAXO_TEST_SF2").ok()?;
        let mut settings = Settings::new();
        let synth = Synth::new(&mut settings);
        // The synth uses its settings until it is deleted, and deleting it
        // after them hangs: keep them for the rest of the test.
        std::mem::forget(settings);
        synth.sfload(&sf2, 1)?;
        synth.program_change(0, 66);
        Some(synth)
    }

    // fluidsynth only frees a voice when it renders the release: pull audio
    // through so voice counts reflect note-offs.
    // (The binding's write_f32 indexes past the end of its buffers.)
    fn render(synth: &Synth) {
        extern "C" {
            fn fluid_synth_write_float(
                synth: *mut std::ffi::c_void,
                len: i32,
                lout: *mut f32,
                loff: i32,
                lincr: i32,
                rout: *mut f32,
                roff: i32,
                rincr: i32,
            ) -> i32;
        }
        const FRAMES: usize = 4410;
        let (mut left, mut right) = (vec![0f32; FRAMES], vec![0f32; FRAMES]);
        let raw = synth.to_raw() as *mut std::ffi::c_void;
        let len = FRAMES as i32;
        let (l, r) = (left.as_mut_ptr(), right.as_mut_ptr());
        unsafe { fluid_synth_write_float(raw, len, l, 0, 1, r, 0, 1) };
    }

    #[test]
    fn beep_plays_then_stops_without_blocking() {
        let Some(synth) = synth() else { return };
        let cues = Cues::new(&synth);
        let before = Instant::now();
        cues.beep(70, 50, Duration::ZERO);
        assert!(before.elapsed() < Duration::from_millis(10));
        assert!(synth.get_active_voice_count() > 0);
        sleep(BEEP_LENGTH + Duration::from_millis(10));
        cues.poll();
        assert!(cues.pending.borrow().is_empty());
        for _ in 0..20 {
            render(&synth);
        }
        assert_eq!(synth.get_active_voice_count(), 0);
    }

    #[test]
    fn delayed_beeps_play_in_order() {
        let Some(synth) = synth() else { return };
        let cues = Cues::new(&synth);
        cues.beep(75, 50, Duration::from_millis(120));
        cues.beep(71, 50, Duration::ZERO);
        // The first beep is on, the second not yet.
        assert_eq!(cues.pending.borrow().len(), 3);
        sleep(Duration::from_millis(130));
        cues.poll();
        // The first has ended and the second started.
        assert_eq!(cues.pending.borrow().len(), 1);
        sleep(BEEP_LENGTH);
        cues.poll();
        assert!(cues.pending.borrow().is_empty());
    }
}
