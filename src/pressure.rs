use std::cmp::min;
use std::error::Error;
use std::thread;
use std::time::Duration;

use log::{debug, error /* info, warn */};

use rppal::i2c::I2c;

// Pressure sensor I2C address
const ADDR_PRESSURE_SENSOR: u16 = 0x4D;

// Readings averaged into the resting baseline at startup.
const BASELINE_SAMPLES: i32 = 32;

// Counts either side of the baseline treated as no breath. Resting noise
// measured on a Zero 2 W spans about +/-8 counts around its mean.
const NOISE_DEADZONE: i32 = 10;

pub struct Pressure {
    i2c: rppal::i2c::I2c,
    baseline: i32,
}

impl Pressure {
    pub fn init() -> Result<Pressure, Box<dyn Error>> {
        debug!("I2C: Configuring bus ...");

        let maybe_i2c = I2c::new();

        let mut i2c = match maybe_i2c {
            Ok(i2c) => i2c,
            Err(e) => {
                error!("Failed to initialize I2C.  Check raspi-config.");
                return Err(Box::new(e));
            }
        };

        // rppal reads the clock speed from a sysfs file that the mainline
        // I2C driver doesn't provide, so it is only logged when available.
        match i2c.clock_speed() {
            Ok(hz) => debug!("I2C: Created on bus {} at {} Hz", i2c.bus(), hz),
            Err(_) => debug!("I2C: Created on bus {}", i2c.bus()),
        }

        // Set the I2C slave address to the device we're communicating with.
        i2c.set_slave_address(ADDR_PRESSURE_SENSOR)?;

        debug!("I2C: slave address set to {}", ADDR_PRESSURE_SENSOR);

        // Average the resting pressure over a few samples: a single reading
        // is noisy enough (about +/-8 counts) that a low first sample leaves
        // the breathless sensor above the baseline, and the main loop then
        // retriggers the note on every tick.
        let mut sum = 0;
        for _ in 0..BASELINE_SAMPLES {
            sum += Pressure::read_io(&mut i2c)?;
            thread::sleep(Duration::from_millis(2));
        }
        let baseline = sum / BASELINE_SAMPLES;

        let sensor = Pressure {
            i2c: i2c,
            baseline: baseline,
        };

        debug!("I2C: baseline set to {}", sensor.baseline);

        Ok(sensor)
    }

    pub fn read(&mut self) -> Result<i32, Box<dyn Error>> {
        let pressure = Pressure::read_io(&mut self.i2c)?;
        // Ignore sensor noise around the baseline, on both sides, so the
        // negative-pressure gesture keeps working.
        let delta = pressure - self.baseline;
        let delta = if delta > NOISE_DEADZONE {
            delta - NOISE_DEADZONE
        } else if delta < -NOISE_DEADZONE {
            delta + NOISE_DEADZONE
        } else {
            0
        };
        // Compress the the range returned by the sensor to 0-127 required
        // for MIDI.  TODO:  Make this configurable
        const PRESSURE_SCALING_FACTOR: i32 = 6;
        Ok(min(delta / PRESSURE_SCALING_FACTOR, 127))
    }

    fn read_io(i2c: &mut rppal::i2c::I2c) -> Result<i32, Box<dyn Error>> {
        let mut reg = [0u8; 2];
        let mut result;
        i2c.read(&mut reg)?;
        result = reg[0] as i32;
        result <<= 8;
        result |= reg[1] as i32;
        result = result - 2048;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    // Import names from outer (for mod tests) scope.
    use super::*;

    use std::thread;
    use std::time::Duration;

    #[test]
    fn init() {
        let mut _sensor = Pressure::init().expect("Failed to initialize pressure sensor");
    }

    #[test]
    fn read() -> Result<(), Box<dyn Error>> {
        let mut sensor = Pressure::init().expect("Failed to initialize pressure sensor");
        let _pressure = sensor.read()?;
        Ok(())
    }

    /* This test is ignored by default because it expects pressure readings to change over time.
    In order to do that, you might need to blow some air into the tube.

    Run as
    cargo test pressure_step -- --ignored --nocapture
    */
    #[test]
    #[ignore]
    fn pressure_step() -> Result<(), Box<dyn Error>> {
        println!("Blow and draw air from the mouthpiece...");
        let mut sensor = Pressure::init().expect("Failed to initialize pressure sensor");
        let mut pressure_positive_detected = false;
        let mut pressure_negative_detected = false;
        for _ in 0..100 {
            let pressure = sensor.read()?;

            const EXPECTED_VARIATION: i32 = 10;

            if pressure > EXPECTED_VARIATION {
                pressure_positive_detected = true;
                println!("+ pressure: {}", pressure);
            }

            if pressure < -EXPECTED_VARIATION {
                pressure_negative_detected = true;
                println!("- pressure: {}", pressure);
            }

            if pressure_negative_detected && pressure_positive_detected {
                break;
            }
            thread::sleep(Duration::from_millis(50))
        }
        assert!(pressure_positive_detected);
        assert!(pressure_negative_detected);

        Ok(())
    }

    /* Test the range of raw pressure readings coming from the sensor.

    Run as
    cargo test pressure_range -- --ignored --nocapture
    */
    #[test]
    #[ignore]
    fn read_io() -> Result<(), Box<dyn Error>> {
        println!("Blow and draw on the mouthpiece...");
        let mut sensor = Pressure::init().expect("Failed to initialize pressure sensor");
        let mut max_val: i32 = 0;
        let mut min_val: i32 = i32::MAX;
        let mut pressure_range_detected = false;
        // Test that we can cover at least 1/4 of the full 12-bit output range of the sensor
        const EXPECTED_PRESSURE_RANGE: i32 = 4096 / 4;
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(50));
            let pressure = Pressure::read_io(&mut sensor.i2c)?;
            println!("pressure: {}, min: {}, max: {}", pressure, min_val, max_val);
            if pressure > max_val {
                max_val = pressure;
            }
            if pressure < min_val {
                min_val = pressure;
            }
            if max_val - min_val > EXPECTED_PRESSURE_RANGE {
                pressure_range_detected = true;
                break;
            }
        }
        assert!(pressure_range_detected);
        Ok(())
    }
}
