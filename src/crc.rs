//! `steps_crc` — design.md §3 decision 17's integrity seal over `Study.steps`.

use crc::{Crc, CRC_32_ISO_HDLC};
use heapless::Vec;

use crate::limits::MAX_STEPS_PER_STUDY;
use crate::study::Step;

/// A single `Step`'s postcard encoding didn't fit the internal scratch
/// buffer. Should be unreachable given `limits` (design.md §3 decision 15),
/// but this returns an error rather than panicking — this crate avoids
/// panics as a matter of course, not just at the FFI boundary (§3 decision 23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTooLargeError;

/// CRC-32 (the common "CRC-32"/Ethernet/zip polynomial) over a `Study`'s
/// `steps`, design.md §3 decision 17. Computed once by whoever submits a
/// `Study` (`embarch-api`, or a human via the CLI); checked again,
/// independently, at both the API<->Core hop and the Core<->dev-bench hop.
///
/// Streams each step's postcard encoding through the CRC digest one at a
/// time rather than buffering the whole `steps` list at once, so a
/// constrained dev-bench MCU only ever needs a stack buffer sized for one
/// `Step`, not for `MAX_STEPS_PER_STUDY` of them.
pub fn steps_crc(steps: &Vec<Step, MAX_STEPS_PER_STUDY>) -> Result<u32, StepTooLargeError> {
    const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
    // Generous margin above the worst-case single encoded `Step` (~600
    // bytes, dominated by `GattOperation::Write`'s 512-byte payload).
    const SCRATCH_LEN: usize = 768;

    let mut digest = CRC32.digest();
    let mut scratch = [0u8; SCRATCH_LEN];
    for step in steps.iter() {
        let encoded = postcard::to_slice(step, &mut scratch).map_err(|_| StepTooLargeError)?;
        digest.update(encoded);
    }
    Ok(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::{Action, BleRole, PowerSampleWindow};

    fn step(name: &str) -> Step {
        Step {
            name: heapless::String::try_from(name).unwrap(),
            action: Action::BleConnect { role: BleRole::Central, target_address: None },
            timeout_ms: 5_000,
            power_sample: Some(PowerSampleWindow { sample_rate_hz: 1_000 }),
            continue_on_fail: false,
        }
    }

    #[test]
    fn same_steps_produce_same_crc() {
        let mut a: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        a.push(step("connect")).unwrap();
        let mut b: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        b.push(step("connect")).unwrap();

        assert_eq!(steps_crc(&a).unwrap(), steps_crc(&b).unwrap());
    }

    #[test]
    fn different_steps_produce_different_crc() {
        let mut a: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        a.push(step("connect")).unwrap();
        let mut b: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        b.push(step("connect-2")).unwrap();

        assert_ne!(steps_crc(&a).unwrap(), steps_crc(&b).unwrap());
    }
}
