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
    last_cmd_key: CommandKeys,
}

impl<'a> Command<'a> {
    pub(crate) fn new(synth: &'a Synth, cues: &'a Cues<'a>, prog_number: i32) -> Self {
        Command {
            synth: synth,
            cues: cues,
            prog_number: prog_number,
            last_cmd_key: CommandKeys::Unmapped,
        }
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
        self.prog_number = max(0, min(127, self.prog_number + change));
        self.synth.program_change(0, self.prog_number);
        info!("New MIDI program number {}", self.prog_number);
        self.cues.beep(53, 60, Duration::ZERO);
    }
}
