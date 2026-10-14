use std::cmp::{max, min};
use std::time::Duration;

use fluidsynth::synth::Synth;
use log::info;

use crate::cues::Cues;

#[derive(Copy, Clone, PartialEq)]
enum CommandKeys {
    ChangeProgUp,
    ChangeProgFastUp,
    ChangeProgDown,
    ChangeProgFastDown,
    Unmapped,
}

fn key2cmdkey(key: u32) -> CommandKeys {
    match key {
        0x10000 => CommandKeys::ChangeProgUp,
        0x90000 => CommandKeys::ChangeProgFastUp,
        0x400000 => CommandKeys::ChangeProgDown,
        0x480000 => CommandKeys::ChangeProgFastDown,
        // 0x800000=> CommandKeys::ChangeVolume,
        _ => CommandKeys::Unmapped,
    }
}

pub(crate) struct Command<'a> {
    synth: &'a Synth,
    cues: &'a Cues<'a>,
    prog_number: i32,
    // The programs the soundfont has in bank 0; empty if unknown.
    programs: Vec<i32>,
    last_cmd_key: CommandKeys,
}

impl<'a> Command<'a> {
    pub(crate) fn new(synth: &'a Synth, cues: &'a Cues<'a>, prog_number: i32) -> Self {
        let programs = crate::synth::programs(synth, 0);
        info!("Soundfont has {} programs in bank 0", programs.len());
        Command {
            synth: synth,
            cues: cues,
            prog_number: prog_number,
            programs: programs,
            last_cmd_key: CommandKeys::Unmapped,
        }
    }
    pub(crate) fn program(&self) -> i32 {
        self.prog_number
    }

    // Select `program` from outside (the control socket), without a beep.
    pub(crate) fn set_program(&mut self, program: i32) {
        self.prog_number = program;
        self.synth.program_change(0, program);
        info!("New MIDI program number {}", program);
    }

    // After a soundfont change: the presets to step through.
    pub(crate) fn reload_programs(&mut self) {
        self.programs = crate::synth::programs(self.synth, 0);
        info!("Soundfont has {} programs in bank 0", self.programs.len());
    }

    pub(crate) fn process(self: &mut Self, key: u32) {
        let cmd_key = key2cmdkey(key);
        if cmd_key == self.last_cmd_key {
            return;
        }
        self.last_cmd_key = cmd_key;

        match cmd_key {
            CommandKeys::ChangeProgUp => self.change_program(1),
            CommandKeys::ChangeProgFastUp => self.change_program(10),
            CommandKeys::ChangeProgDown => self.change_program(-1),
            CommandKeys::ChangeProgFastDown => self.change_program(-10),
            _ => (),
        };
    }

    fn change_program(self: &mut Self, change: i32) {
        self.prog_number = step(&self.programs, self.prog_number, change);
        self.synth.program_change(0, self.prog_number);
        info!("New MIDI program number {}", self.prog_number);
        self.cues.beep(53, 60, Duration::ZERO);
    }
}

// The program `change` steps from `current`: the nearest one that exists in
// that direction, at least `change` away when there is one, else the last
// one that way. Without a list, any of 0-127.
fn step(programs: &[i32], current: i32, change: i32) -> i32 {
    let target = current + change;
    if programs.is_empty() {
        return max(0, min(127, target));
    }
    if change > 0 {
        let after = programs.iter().copied().filter(|&p| p > current);
        let next = after.clone().find(|&p| p >= target);
        next.or(after.last()).unwrap_or(current)
    } else {
        let before = programs.iter().rev().copied().filter(|&p| p < current);
        let next = before.clone().find(|&p| p <= target);
        next.or(before.last()).unwrap_or(current)
    }
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn full_bank_steps_like_before() {
        let all: Vec<i32> = (0..128).collect();
        assert_eq!(step(&all, 66, 1), 67);
        assert_eq!(step(&all, 66, -10), 56);
        assert_eq!(step(&all, 127, 1), 127);
        assert_eq!(step(&all, 3, -10), 0);
        assert_eq!(step(&[], 66, 10), 76);
        assert_eq!(step(&[], 125, 10), 127);
    }

    #[test]
    fn sparse_bank_skips_missing_programs() {
        let some = [0, 3, 5, 6, 7, 64, 66];
        assert_eq!(step(&some, 0, 1), 3);
        assert_eq!(step(&some, 7, 1), 64);
        assert_eq!(step(&some, 66, 1), 66);
        assert_eq!(step(&some, 64, -1), 7);
        assert_eq!(step(&some, 0, 10), 64);
        assert_eq!(step(&some, 66, -10), 7);
        assert_eq!(step(&some, 5, -10), 0);
        // A start that isn't in the list moves onto it.
        assert_eq!(step(&some, 1, 1), 3);
        assert_eq!(step(&some, 67, -1), 66);
    }
}
