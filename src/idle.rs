// Idle mode, for battery: after a while with no breath and no key changes
// the main loop closes the audio stream (which shuts the amp down) and
// slows its tick; the first breath or key change wakes it. Fingering a
// note before blowing it, as players do, wakes the audio in time.
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
pub enum Change {
    None,
    Sleep,
    Wake,
}

pub struct Idle {
    // None: never go idle.
    after: Option<Duration>,
    last_active: Instant,
    last_keys: u32,
    idle: bool,
}

impl Idle {
    pub fn new(after: Option<Duration>, now: Instant) -> Self {
        Idle {
            after,
            last_active: now,
            last_keys: 0,
            idle: false,
        }
    }

    // Call every tick with what was read. `pressure` counts either way, so
    // the negative-pressure gesture wakes the instrument as well.
    pub fn update(&mut self, now: Instant, keys: u32, pressure: i32) -> Change {
        let active = keys != self.last_keys || pressure != 0;
        self.last_keys = keys;
        if active {
            self.last_active = now;
            if self.idle {
                self.idle = false;
                return Change::Wake;
            }
            return Change::None;
        }
        match self.after {
            Some(after) if !self.idle && now - self.last_active >= after => {
                self.idle = true;
                Change::Sleep
            }
            _ => Change::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn sleeps_after_quiet_and_wakes_on_activity() {
        let t0 = Instant::now();
        let mut idle = Idle::new(Some(30 * S), t0);
        assert_eq!(idle.update(t0 + 29 * S, 0, 0), Change::None);
        assert_eq!(idle.update(t0 + 30 * S, 0, 0), Change::Sleep);
        assert_eq!(idle.update(t0 + 40 * S, 0, 0), Change::None);
        // A key change wakes it; holding the keys afterwards is no activity.
        assert_eq!(idle.update(t0 + 50 * S, 0x10, 0), Change::Wake);
        assert_eq!(idle.update(t0 + 79 * S, 0x10, 0), Change::None);
        assert_eq!(idle.update(t0 + 80 * S, 0x10, 0), Change::Sleep);
        // So do breath and the negative-pressure gesture.
        assert_eq!(idle.update(t0 + 81 * S, 0x10, 5), Change::Wake);
        assert_eq!(idle.update(t0 + 111 * S, 0x10, 0), Change::Sleep);
        assert_eq!(idle.update(t0 + 112 * S, 0x10, -20), Change::Wake);
    }

    #[test]
    fn playing_keeps_it_awake() {
        let t0 = Instant::now();
        let mut idle = Idle::new(Some(30 * S), t0);
        for i in 0..100 {
            assert_eq!(idle.update(t0 + i * S, 0, 40), Change::None);
        }
    }

    #[test]
    fn never_sleeps_when_disabled() {
        let t0 = Instant::now();
        let mut idle = Idle::new(None, t0);
        assert_eq!(idle.update(t0 + 3600 * S, 0, 0), Change::None);
    }
}
