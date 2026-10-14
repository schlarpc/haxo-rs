// Bluetooth LE MIDI: a GATT service (the BLE MIDI spec's UUIDs) that phones
// and DAWs connect to, as BlueZ's own MIDI plugin only connects out.
//
// The main loop hands messages over without blocking; a thread of its own,
// at normal priority, batches them into BLE MIDI packets. A link carries a
// packet per connection event (7.5-50 ms), so packets go out at most every
// PACKET_INTERVAL, and breath values in a row collapse to the latest: the
// queue in the Bluetooth stack never builds up behind the notes (#41).
use std::thread;
use std::time::{Duration, Instant};

use bluer::adv::Advertisement;
use bluer::gatt::local::{
    characteristic_control, Application, Characteristic, CharacteristicControlEvent,
    CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicRead, CharacteristicWrite,
    CharacteristicWriteMethod, Service,
};
use bluer::gatt::CharacteristicWriter;
use futures::{FutureExt, StreamExt};
use log::{info, warn};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

const SERVICE_UUID: bluer::Uuid = bluer::Uuid::from_u128(0x03b80e5a_ede8_4b33_a751_6ce34ec4c700);
const CHARACTERISTIC_UUID: bluer::Uuid =
    bluer::Uuid::from_u128(0x7772e5db_3868_4112_a1a9_f2669d106bf3);

const PACKET_INTERVAL: Duration = Duration::from_millis(10);
// Messages the main loop may get ahead of the BLE thread by.
const QUEUE: usize = 256;
const CC_BREATH: u8 = 2;

// A MIDI channel message and when the main loop sent it.
type Message = (Instant, [u8; 3]);

pub struct BleMidi {
    tx: mpsc::Sender<Message>,
    dropped: u64,
}

impl BleMidi {
    // Start the service on the default adapter under `name`. The thread
    // starts before the main loop turns realtime, so it doesn't inherit that.
    pub fn start(name: &str) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE);
        let name = name.to_string();
        thread::Builder::new()
            .name("ble-midi".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                if let Err(e) = rt.block_on(serve(name, rx)) {
                    warn!("Bluetooth MIDI stopped: {}", e);
                }
            })
            .expect("BLE MIDI thread");
        BleMidi { tx, dropped: 0 }
    }

    pub fn send(&mut self, msg: &[u8]) {
        let mut m = [0u8; 3];
        let n = msg.len().min(3);
        m[..n].copy_from_slice(&msg[..n]);
        if self.tx.try_send((Instant::now(), m)).is_err() {
            self.dropped += 1;
            if self.dropped.is_power_of_two() {
                warn!("Bluetooth MIDI: {} messages dropped", self.dropped);
            }
        }
    }
}

async fn serve(name: String, mut rx: mpsc::Receiver<Message>) -> bluer::Result<()> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    let (char_control, char_handle) = characteristic_control();
    let app = Application {
        services: vec![Service {
            uuid: SERVICE_UUID,
            primary: true,
            characteristics: vec![Characteristic {
                uuid: CHARACTERISTIC_UUID,
                // The spec wants read (answered empty), write without
                // response and notify. Incoming MIDI is ignored.
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|_| async { Ok(Vec::new()) }.boxed()),
                    ..Default::default()
                }),
                write: Some(CharacteristicWrite {
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Fun(Box::new(|_, _| {
                        async { Ok(()) }.boxed()
                    })),
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Io,
                    ..Default::default()
                }),
                control_handle: char_handle,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let _app = adapter.serve_gatt_application(app).await?;
    let _adv = adapter
        .advertise(Advertisement {
            service_uuids: vec![SERVICE_UUID].into_iter().collect(),
            discoverable: Some(true),
            local_name: Some(name.clone()),
            ..Default::default()
        })
        .await?;
    info!("Bluetooth MIDI: advertising as {}", name);

    let start = Instant::now();
    let mut char_control = Box::pin(char_control);
    let mut writer: Option<CharacteristicWriter> = None;
    let mut batch: Vec<Message> = Vec::new();
    let mut last_sent = Instant::now() - PACKET_INTERVAL;
    loop {
        let due = tokio::time::Instant::from_std(last_sent + PACKET_INTERVAL);
        tokio::select! {
            evt = char_control.next() => match evt {
                Some(CharacteristicControlEvent::Notify(w)) => {
                    info!("Bluetooth MIDI: {} subscribed, MTU {}", w.device_address(), w.mtu());
                    // Release anything a previous connection left sounding.
                    batch.clear();
                    batch.push((Instant::now(), [0xB0, 123, 0]));
                    writer = Some(w);
                }
                Some(CharacteristicControlEvent::Write(_)) => (),
                None => break,
            },
            msg = rx.recv() => match msg {
                Some(m) => {
                    if writer.is_some() {
                        add(&mut batch, m);
                    }
                }
                None => break,
            },
            _ = tokio::time::sleep_until(due), if !batch.is_empty() && writer.is_some() => {
                let w = writer.as_mut().unwrap();
                let max = w.mtu().saturating_sub(3).max(5);
                let mut ok = true;
                for packet in encode(&batch, start, max) {
                    if let Err(e) = w.write_all(&packet).await {
                        info!("Bluetooth MIDI: {} gone ({})", w.device_address(), e);
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    writer = None;
                }
                batch.clear();
                last_sent = Instant::now();
            }
        }
    }
    Ok(())
}

// Add a message to the batch. A breath value right after another replaces
// it; one before a note stays, so the note starts at that breath, and the
// timestamps stay in order.
fn add(batch: &mut Vec<Message>, m: Message) {
    let is_breath = |b: &[u8; 3]| b[0] & 0xF0 == 0xB0 && b[1] == CC_BREATH;
    if let Some(last) = batch.last_mut() {
        if is_breath(&m.1) && is_breath(&last.1) && last.1[0] == m.1[0] {
            *last = m;
            return;
        }
    }
    batch.push(m);
}

fn message_len(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 2,
        _ => 3,
    }
}

// BLE MIDI packets of at most `max` bytes: a header byte with the high 6
// bits of a 13-bit millisecond timestamp, then each message after a byte
// with the low 7 bits.
fn encode(batch: &[Message], start: Instant, max: usize) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut packet: Vec<u8> = Vec::new();
    for (at, msg) in batch {
        let ms = at.saturating_duration_since(start).as_millis() as u16 & 0x1FFF;
        let msg = &msg[..message_len(msg[0])];
        if !packet.is_empty() && packet.len() + 1 + msg.len() > max {
            packets.push(std::mem::take(&mut packet));
        }
        if packet.is_empty() {
            packet.push(0x80 | (ms >> 7) as u8);
        }
        packet.push(0x80 | (ms & 0x7F) as u8);
        packet.extend_from_slice(msg);
    }
    if !packet.is_empty() {
        packets.push(packet);
    }
    packets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn encodes_timestamps_and_messages() {
        let t0 = Instant::now();
        let batch = [
            (at(t0, 300), [0x90, 60, 127]),
            (at(t0, 301), [0xB0, 2, 40]),
            (at(t0, 302), [0xC0, 5, 0]),
        ];
        let p = encode(&batch, t0, 20);
        // 300 ms = 0b10_0101100: header 0x80|2, low 0x80|0x2C.
        assert_eq!(
            p,
            vec![vec![
                0x82, 0xAC, 0x90, 60, 127, 0xAD, 0xB0, 2, 40, 0xAE, 0xC0, 5
            ]]
        );
    }

    #[test]
    fn timestamps_wrap_at_13_bits() {
        let t0 = Instant::now();
        let p = encode(&[(at(t0, 8192 + 5), [0x80, 60, 0])], t0, 20);
        assert_eq!(p, vec![vec![0x80, 0x85, 0x80, 60, 0]]);
    }

    #[test]
    fn splits_at_the_mtu() {
        let t0 = Instant::now();
        let batch: Vec<Message> = (0..5).map(|i| (t0, [0x90, 60 + i, 100])).collect();
        // 20 bytes: header + 4 x (timestamp + 3) = 17; the fifth goes next.
        let p = encode(&batch, t0, 20);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].len(), 17);
        assert_eq!(p[1], vec![0x80, 0x80, 0x90, 64, 100]);
    }

    #[test]
    fn merges_consecutive_breath_only() {
        let t0 = Instant::now();
        let mut batch = Vec::new();
        add(&mut batch, (at(t0, 1), [0xB0, 2, 5]));
        add(&mut batch, (at(t0, 2), [0xB0, 2, 10]));
        add(&mut batch, (at(t0, 3), [0x90, 60, 127]));
        add(&mut batch, (at(t0, 4), [0xB0, 2, 20]));
        add(&mut batch, (at(t0, 5), [0xB0, 2, 30]));
        add(&mut batch, (at(t0, 6), [0xB0, 7, 99]));
        let msgs: Vec<[u8; 3]> = batch.iter().map(|(_, m)| *m).collect();
        assert_eq!(
            msgs,
            vec![[0xB0, 2, 10], [0x90, 60, 127], [0xB0, 2, 30], [0xB0, 7, 99]]
        );
        let times: Vec<Instant> = batch.iter().map(|(t, _)| *t).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
    }
}
