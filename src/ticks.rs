// Main loop timing: how late ticks wake up and how long each one's work
// takes, summarized every REPORT_TICKS at debug level under the "ticks"
// target (RUST_LOG=ticks=debug).
use log::{debug, log_enabled, Level};
use std::time::{Duration, Instant};

const REPORT_TICKS: u32 = 5_000;

pub struct TickStats {
    period: Duration,
    last_wake: Option<Instant>,
    ticks: u32,
    max_interval: Duration,
    max_busy: Duration,
    // Intervals over 1.5x, 2.5x and 5x the period.
    late: [u32; 3],
}

impl TickStats {
    pub fn new(period: Duration) -> Self {
        TickStats {
            period,
            last_wake: None,
            ticks: 0,
            max_interval: Duration::ZERO,
            max_busy: Duration::ZERO,
            late: [0; 3],
        }
    }

    // Call when a tick wakes the loop.
    pub fn wake(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_wake {
            let interval = now - last;
            self.max_interval = self.max_interval.max(interval);
            for (i, x) in [3u32, 5, 10].iter().enumerate() {
                if interval > self.period * *x / 2 {
                    self.late[i] += 1;
                }
            }
        }
        self.last_wake = Some(now);
    }

    // Call before waiting for the next tick.
    pub fn idle(&mut self) {
        let Some(woke) = self.last_wake else { return };
        self.max_busy = self.max_busy.max(woke.elapsed());
        self.ticks += 1;
        if self.ticks < REPORT_TICKS {
            return;
        }
        if log_enabled!(target: "ticks", Level::Debug) {
            debug!(
                target: "ticks",
                "{} ticks: max interval {} us, over 1.5/2.5/5 periods {}/{}/{}, max busy {} us",
                self.ticks,
                self.max_interval.as_micros(),
                self.late[0],
                self.late[1],
                self.late[2],
                self.max_busy.as_micros()
            );
        }
        self.ticks = 0;
        self.max_interval = Duration::ZERO;
        self.max_busy = Duration::ZERO;
        self.late = [0; 3];
    }
}
