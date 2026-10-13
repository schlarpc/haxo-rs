use std::error::Error;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use gpiocdev::line::{Bias, Value, Values};
use gpiocdev::Request;

use static_assertions::const_assert;

// BCM pin numbering, which is also the line offset on gpiochip0
const ROWS: [u8; 8] = [13, 12, 16, 17, 20, 22, 23, 24];
const COLS: [u8; 3] = [25, 26, 27];

const ROW_PULL_DOWN_TIME_US: u64 = 10;

const GPIO_CHIP: &str = "/dev/gpiochip0";

// One line request for the whole matrix, held for the life of the process:
// rows driven high (idle), columns as inputs with pull-ups. This goes
// through the kernel's GPIO character device, so it needs neither
// /dev/gpiomem (rpi kernel only) nor /dev/mem, and nothing is mapped or
// reopened per scan.
static MATRIX: OnceLock<Request> = OnceLock::new();

fn offsets(pins: &[u8]) -> Vec<u32> {
    pins.iter().map(|p| *p as u32).collect()
}

pub fn init_io() -> Result<(), Box<dyn Error>> {
    if MATRIX.get().is_some() {
        return Ok(());
    }
    let req = Request::builder()
        .on_chip(GPIO_CHIP)
        .with_consumer("haxo-keyscan")
        .with_lines(&offsets(&ROWS))
        .as_output(Value::Active)
        .with_lines(&offsets(&COLS))
        .as_input()
        .with_bias(Bias::PullUp)
        .request()?;
    // A concurrent init (only tests do that) keeps the first request.
    let _ = MATRIX.set(req);
    Ok(())
}

fn get_bit_at(input: u32, n: u8) -> bool {
    if n < 32 {
        input & (1 << n) != 0
    } else {
        false
    }
}

fn set_bit_at(output: &mut u32, n: u8) {
    if n < 32 {
        *output |= 1 << n;
    }
}

pub fn scan() -> Result<u32, Box<dyn Error>> {
    const_assert!(ROWS.len() + COLS.len() <= 32);
    let req = MATRIX.get().ok_or("keyscan::init_io() was not called")?;
    let col_offsets = offsets(&COLS);
    let mut cols = Values::from_offsets(&col_offsets);
    let mut key_idx = 0;
    // a bit if set if the corresponding key is pressed
    let mut keymap: u32 = 0;
    for row in &ROWS {
        let row = *row as u32;
        req.set_value(row, Value::Inactive)?;
        thread::sleep(Duration::from_micros(ROW_PULL_DOWN_TIME_US));

        req.values(&mut cols)?;
        for col in &col_offsets {
            if cols.get(*col) == Some(Value::Inactive) {
                set_bit_at(&mut keymap, key_idx);
            }
            key_idx += 1;
        }
        req.set_value(row, Value::Active)?;
    }
    Ok(keymap)
}

#[allow(dead_code)]
pub fn debug_print(keys: u32) {
    println!("");
    print!("  ");
    for _col in &COLS {
        print!("==");
    }
    println!("");
    print!("   ");
    for (i, _col) in COLS.iter().enumerate() {
        print!("{} ", i);
    }
    println!("");
    print!("  ");
    for _col in &COLS {
        print!("==");
    }
    println!("");
    for (ir, _) in ROWS.iter().enumerate() {
        for (ic, _) in COLS.iter().enumerate() {
            if ic == 0 {
                print!("{}: ", ir);
            }
            let key = get_bit_at(keys, (ir * COLS.len() + ic) as u8);
            print!("{} ", if key { "x" } else { "o" });
        }
        println!("");
    }
}

#[cfg(test)]
mod tests {
    // Import names from outer (for mod tests) scope.
    use super::*;

    use std::thread;
    use std::time::Duration;

    #[test]
    fn init() -> Result<(), Box<dyn Error>> {
        init_io().expect("Failed to initialize scan GPIO");
        Ok(())
    }

    #[test]
    fn read() -> Result<(), Box<dyn Error>> {
        init_io().expect("Failed to initialize scan GPIO");
        let _keys = scan()?;
        Ok(())
    }

    /* This test is ignored by default because it requires user interaction.
    In order to pass, all keys must be pressed at least once.

    Run as
    cargo test all_keys -- --ignored --nocapture
    */
    #[test]
    #[ignore]
    fn all_keys() -> Result<(), Box<dyn Error>> {
        const NUM_KEYS: u32 = 22;
        println!("Press all the keys at least once, in any order...");
        init_io().expect("Failed to initialize scan GPIO");
        let mut detected_keys: u32 = 0;
        let mut last_keys: u32 = 0;
        for _ in 0..5000 {
            let keys = scan()?;
            thread::sleep(Duration::from_millis(50));
            detected_keys |= keys;
            if last_keys != keys {
                println!(
                    "{:02}/{}: detected_keys: {:x} keys: {:x} ({}) ",
                    detected_keys.count_ones(),
                    NUM_KEYS,
                    detected_keys,
                    keys,
                    keys
                );
                last_keys = keys;
                debug_print(detected_keys);
            }
            if detected_keys.count_ones() == NUM_KEYS {
                return Ok(());
            }
        }
        Err("Not all keys were detected")?
    }
}
