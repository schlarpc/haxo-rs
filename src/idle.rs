// Idle mode, for battery: after a while with no breath and no key changes
// the main loop closes the audio stream (which shuts the amp down) and
// slows its tick; the first breath or key change wakes it. Fingering a
// note before blowing it, as players do, wakes the audio in time.
use log::{debug, log_enabled, Level};
use std::time::{Duration, Instant};

const REPORT_UPDATES: u32 = 5_000;
// The pressure sensor reads a spike past the noise dead zone a dozen times
// in 10 s at rest, always for one tick; breath lasts longer.
const PRESSURE_TICKS: u32 = 2;

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
    // Ticks in a row with pressure.
    pressure_run: u32,
    idle: bool,
    // What counted as activity since the last report (RUST_LOG=idle=debug).
    updates: u32,
    key_changes: u32,
    keys_seen: u32,
    pressure_ticks: u32,
    pressure_range: (i32, i32),
}

impl Idle {
    pub fn new(after: Option<Duration>, now: Instant) -> Self {
        Idle {
            after,
            last_active: now,
            last_keys: 0,
            pressure_run: 0,
            idle: false,
            updates: 0,
            key_changes: 0,
            keys_seen: 0,
            pressure_ticks: 0,
            pressure_range: (0, 0),
        }
    }

    // Call every tick with what was read. `pressure` counts either way, so
    // the negative-pressure gesture wakes the instrument as well.
    pub fn update(&mut self, now: Instant, keys: u32, pressure: i32) -> Change {
        self.pressure_run = if pressure != 0 {
            self.pressure_run + 1
        } else {
            0
        };
        let active = keys != self.last_keys || self.pressure_run >= PRESSURE_TICKS;
        self.count(keys, pressure);
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

impl Idle {
    fn count(&mut self, keys: u32, pressure: i32) {
        if keys != self.last_keys {
            self.key_changes += 1;
            self.keys_seen |= keys ^ self.last_keys;
        }
        if pressure != 0 {
            self.pressure_ticks += 1;
            let (lo, hi) = self.pressure_range;
            self.pressure_range = (lo.min(pressure), hi.max(pressure));
        }
        self.updates += 1;
        if self.updates < REPORT_UPDATES {
            return;
        }
        if log_enabled!(target: "idle", Level::Debug) {
            debug!(
                target: "idle",
                "{} ticks: {} key changes (bits {:#x}), {} with pressure ({}..{})",
                self.updates,
                self.key_changes,
                self.keys_seen,
                self.pressure_ticks,
                self.pressure_range.0,
                self.pressure_range.1
            );
        }
        self.updates = 0;
        self.key_changes = 0;
        self.keys_seen = 0;
        self.pressure_ticks = 0;
        self.pressure_range = (0, 0);
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
        // So do breath and the negative-pressure gesture, from their
        // second tick on.
        assert_eq!(idle.update(t0 + 81 * S, 0x10, 5), Change::None);
        assert_eq!(idle.update(t0 + 81 * S, 0x10, 5), Change::Wake);
        assert_eq!(idle.update(t0 + 111 * S, 0x10, 0), Change::Sleep);
        assert_eq!(idle.update(t0 + 112 * S, 0x10, -20), Change::None);
        assert_eq!(idle.update(t0 + 112 * S, 0x10, -20), Change::Wake);
    }

    #[test]
    fn single_pressure_spikes_are_not_activity() {
        let t0 = Instant::now();
        let mut idle = Idle::new(Some(30 * S), t0);
        for i in 0..30 {
            assert_eq!(idle.update(t0 + i * S, 0, (i % 2) as i32), Change::None);
        }
        assert_eq!(idle.update(t0 + 30 * S, 0, 0), Change::Sleep);
        assert_eq!(idle.update(t0 + 31 * S, 0, 1), Change::None);
        assert_eq!(idle.update(t0 + 32 * S, 0, 0), Change::None);
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
