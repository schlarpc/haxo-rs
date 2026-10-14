// Main loop timing: how late ticks wake up and how long each one's work
// takes, summarized every REPORT_TICKS at debug level under the "ticks"
// target (RUST_LOG=ticks=debug).
use log::{debug, info, log_enabled, warn, Level};
use std::thread;
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

    // Change the period the intervals are measured against.
    pub fn set_period(&mut self, period: Duration) {
        self.period = period;
        self.last_wake = None;
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

// The loop's own fixed-rate timer. It runs in the calling thread, so the
// wakeups get that thread's scheduling priority.
pub struct Ticker {
    period: Duration,
    next: Instant,
}

impl Ticker {
    pub fn new(period: Duration) -> Self {
        Ticker {
            period,
            next: Instant::now() + period,
        }
    }

    // Change the period from the next tick on.
    pub fn set_period(&mut self, period: Duration) {
        self.next = Instant::now() + period;
        self.period = period;
    }

    // Sleep until the next tick. Ticks missed while the loop ran late are
    // skipped rather than run back to back.
    pub fn wait(&mut self) {
        let now = Instant::now();
        if self.next > now {
            thread::sleep(self.next - now);
        }
        self.next += self.period;
        let now = Instant::now();
        while self.next <= now {
            self.next += self.period;
        }
    }
}

// Below fluidsynth's audio thread (audio.realtime-prio 99), above everything
// else: a late tick delays note onsets.
const LOOP_PRIORITY: i32 = 80;

// Run the calling thread at realtime priority and keep the process in RAM,
// so neither other load nor paging delays a tick. Both need root (or
// CAP_SYS_NICE and CAP_IPC_LOCK); without them haxo runs as before.
pub fn make_realtime() {
    let param = libc::sched_param {
        sched_priority: LOOP_PRIORITY,
    };
    if unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) } != 0 {
        warn!(
            "Main loop not realtime: {}",
            std::io::Error::last_os_error()
        );
    } else {
        info!("Main loop at SCHED_FIFO priority {}", LOOP_PRIORITY);
    }
    if unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) } != 0 {
        warn!("Memory not locked: {}", std::io::Error::last_os_error());
    }
}
