use ::alsa::rawmidi::Rawmidi;
use ::alsa::Direction;
use midir::{MidiOutput, MidiOutputConnection, MidiOutputPort};

use std::error::Error;
use std::io::Write;
use std::time::{Duration, Instant};

use log::{info, warn};

// A BLE MIDI link carries a packet every 15-50 ms, and breath changes on
// almost every 2 ms tick; sent as they come, the messages queue up and the
// notes behind them arrive late. Breath goes out at most this often, always
// with the latest value.
const BREATH_INTERVAL: Duration = Duration::from_millis(10);

enum Conn {
    // A rawmidi device, opened non-blocking: when the receiver stops
    // reading, messages are dropped instead of stalling the main loop.
    Raw(Rawmidi),
    // An ALSA sequencer port, when there is no such device.
    Seq(MidiOutputConnection),
}

pub struct MidiOut {
    // None when there was nothing to connect to: the synth still plays.
    conn_out: Option<Conn>,
    // Messages dropped because the rawmidi buffer was full.
    dropped: u64,
    // The same messages over Bluetooth LE MIDI, when it is on.
    #[cfg(feature = "ble")]
    ble: Option<crate::ble::BleMidi>,
    // Breath CC value not yet sent, and when one was last sent.
    breath_pending: Option<u8>,
    breath_sent_at: Option<Instant>,
}

impl MidiOut {
    // Send to the rawmidi `device` (e.g. "hw:CARD=haxophone", the USB
    // gadget's MIDI function), or when it can't be opened, to a sequencer
    // port as upstream does.
    pub fn new(device: &str, #[allow(unused)] ble_name: Option<&str>) -> Self {
        let conn_out = match Rawmidi::new(device, Direction::Playback, true) {
            Ok(raw) => {
                info!("MIDI out to {}", device);
                Some(Conn::Raw(raw))
            }
            Err(e) => {
                info!("No MIDI device {} ({}), trying sequencer ports", device, e);
                match MidiOut::connect_seq() {
                    Ok(conn) => Some(Conn::Seq(conn)),
                    Err(e) => {
                        warn!("MIDI out disabled: {}", e);
                        None
                    }
                }
            }
        };
        let mut midi = MidiOut {
            conn_out,
            dropped: 0,
            #[cfg(feature = "ble")]
            ble: ble_name.map(crate::ble::BleMidi::start),
            breath_pending: None,
            breath_sent_at: None,
        };
        // Release anything a previous run left sounding on the receiver.
        const ALL_NOTES_OFF: u8 = 123;
        midi.send(&[0xB0, ALL_NOTES_OFF, 0]);
        midi
    }

    fn connect_seq() -> Result<MidiOutputConnection, Box<dyn Error>> {
        let midi_out = MidiOutput::new("Midi Output")?;

        let out_ports = midi_out.ports();

        let out_port: &MidiOutputPort = match out_ports.len() {
            0 => return Err("no output port found".into()),
            1 => {
                info!(
                    "Choosing the only available midi output port: {}",
                    midi_out.port_name(&out_ports[0]).unwrap()
                );
                &out_ports[0]
            }
            _ => {
                info!("\nAvailable output ports:");
                let mut i = 0;
                for p in out_ports.iter() {
                    info!("{}: {}", i, midi_out.port_name(p).unwrap());
                    i += 1;
                }
                info!("Picked the last one");
                &out_ports[i - 1]
            }
        };
        Ok(midi_out.connect(out_port, "haxophone")?)
    }

    fn send(&mut self, msg: &[u8]) {
        #[cfg(feature = "ble")]
        if let Some(ble) = self.ble.as_mut() {
            ble.send(msg);
        }
        match self.conn_out.as_mut() {
            Some(Conn::Raw(raw)) => {
                // Whole messages only: a partial write would leave the
                // receiver parsing the next message from its middle.
                let room = raw.status().map(|s| s.get_avail()).unwrap_or(0);
                if room < msg.len() || raw.io().write(msg).is_err() {
                    self.dropped += 1;
                    if self.dropped.is_power_of_two() {
                        warn!("MIDI out: {} messages dropped", self.dropped);
                    }
                }
            }
            Some(Conn::Seq(conn)) => {
                let _ = conn.send(msg);
            }
            None => (),
        }
    }

    pub fn noteon(&mut self, note: i32, vel: i32) {
        const NOTE_ON_MSG: u8 = 0x90;
        // The receiver should start the note at the current breath.
        self.flush_breath();
        self.send(&[NOTE_ON_MSG, note as u8, vel as u8]);
    }
    #[allow(dead_code)]
    pub fn cc(&mut self, msg: i32, val: i32) {
        const CC_MSG: u8 = 0xB0;
        self.send(&[CC_MSG, msg as u8, val as u8]);
    }
    pub fn noteoff(&mut self, note: i32) {
        const NOTE_OFF_MSG: u8 = 0x80;
        self.flush_breath();
        self.send(&[NOTE_OFF_MSG, note as u8, 0u8]);
    }

    // Breath (CC#2), rate-limited to BREATH_INTERVAL. Call poll() every
    // tick so a held-back value goes out once the interval has passed.
    pub fn breath(&mut self, val: i32) {
        self.breath_pending = Some(val as u8);
        self.poll();
    }
    pub fn poll(&mut self) {
        let due = match self.breath_sent_at {
            Some(t) => t.elapsed() >= BREATH_INTERVAL,
            None => true,
        };
        if due {
            self.flush_breath();
        }
    }
    fn flush_breath(&mut self) {
        const MIDI_CC_BREATH: u8 = 2;
        if let Some(val) = self.breath_pending.take() {
            self.send(&[0xB0, MIDI_CC_BREATH, val]);
            self.breath_sent_at = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {

    use std::thread::sleep;
    use std::time::Duration;
    use test_log::test;

    use super::*;

    #[test]
    fn test_new() -> Result<(), Box<dyn Error>> {
        MidiOut::new("hw:CARD=haxophone", None);
        Ok(())
    }

    #[test]
    fn test_note() -> Result<(), Box<dyn Error>> {
        let mut midi = MidiOut::new("hw:CARD=haxophone", None);
        midi.noteon(66, 100);
        sleep(Duration::from_millis(1000));
        midi.noteoff(66);
        Ok(())
    }

    #[test]
    fn test_cc() -> Result<(), Box<dyn Error>> {
        let mut midi = MidiOut::new("hw:CARD=haxophone", None);
        midi.noteon(66, 100);
        sleep(Duration::from_millis(1000));
        const MIDI_CC_VOLUME: i32 = 7;
        midi.cc(MIDI_CC_VOLUME, 10);
        sleep(Duration::from_millis(1000));
        midi.cc(MIDI_CC_VOLUME, 100);
        sleep(Duration::from_millis(1000));
        midi.cc(MIDI_CC_VOLUME, 10);
        sleep(Duration::from_millis(1000));
        midi.noteoff(66);
        Ok(())
    }
}
