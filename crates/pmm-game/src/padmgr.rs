/// N64 controller button flags + 2S2H extended Ocarina/custom bits (`BenPort.h`).
pub mod n64_btn {
    pub const A_BUTTON: u32 = 0x8000;
    pub const B_BUTTON: u32 = 0x4000;
    pub const Z_TRIG: u32 = 0x2000;
    pub const START_BUTTON: u32 = 0x1000;
    pub const U_JPAD: u32 = 0x0800;
    pub const D_JPAD: u32 = 0x0400;
    pub const L_JPAD: u32 = 0x0200;
    pub const R_JPAD: u32 = 0x0100;
    pub const L_TRIG: u32 = 0x0020;
    pub const R_TRIG: u32 = 0x0010;
    pub const U_CBUTTONS: u32 = 0x0008;
    pub const D_CBUTTONS: u32 = 0x0004;
    pub const L_CBUTTONS: u32 = 0x0002;
    pub const R_CBUTTONS: u32 = 0x0001;

    pub const BTN_CUSTOM_OCARINA_NOTE_D4: u32 = 0x0001_0000;
    pub const BTN_CUSTOM_OCARINA_NOTE_F4: u32 = 0x0002_0000;
    pub const BTN_CUSTOM_OCARINA_NOTE_A4: u32 = 0x0004_0000;
    pub const BTN_CUSTOM_OCARINA_NOTE_B4: u32 = 0x0008_0000;
    pub const BTN_CUSTOM_OCARINA_NOTE_D5: u32 = 0x0010_0000;
}

/// PSP `CtrlButtons` bit masks (`psp/src/sys/ctrl.rs`).
pub mod psp_btn {
    pub const SELECT: u32 = 0x0000_0001;
    pub const START: u32 = 0x0000_0008;
    pub const UP: u32 = 0x0000_0010;
    pub const RIGHT: u32 = 0x0000_0020;
    pub const DOWN: u32 = 0x0000_0040;
    pub const LEFT: u32 = 0x0000_0080;
    pub const LTRIGGER: u32 = 0x0000_0100;
    pub const RTRIGGER: u32 = 0x0000_0200;
    pub const TRIANGLE: u32 = 0x0000_1000;
    pub const CIRCLE: u32 = 0x0000_2000;
    pub const CROSS: u32 = 0x0000_4000;
    pub const SQUARE: u32 = 0x0000_8000;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadInput {
    pub cur_buttons: u32,
    pub press_buttons: u32,
    pub rel_buttons: u32,
    pub stick_x: i8,
    pub stick_y: i8,
}

impl PadInput {
    #[inline]
    pub const fn pressed(&self, mask: u32) -> bool {
        (self.press_buttons & mask) != 0
    }

    #[inline]
    pub const fn held(&self, mask: u32) -> bool {
        (self.cur_buttons & mask) != 0
    }
}

#[derive(Default)]
pub struct PadMgr {
    prev_buttons: u32,
    pub state: PadInput,
}

impl PadMgr {
    pub const DEADZONE: i32 = 14;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_from_psp(&mut self, psp_buttons: u32, lx: u8, ly: u8) -> PadInput {
        let mut n64 = 0u32;
        if (psp_buttons & psp_btn::CROSS) != 0 {
            n64 |= n64_btn::A_BUTTON;
        }
        if (psp_buttons & (psp_btn::CIRCLE | psp_btn::SQUARE)) != 0 {
            n64 |= n64_btn::B_BUTTON;
        }
        if (psp_buttons & psp_btn::START) != 0 {
            n64 |= n64_btn::START_BUTTON;
        }
        if (psp_buttons & psp_btn::LTRIGGER) != 0 {
            n64 |= n64_btn::Z_TRIG;
        }
        if (psp_buttons & psp_btn::RTRIGGER) != 0 {
            n64 |= n64_btn::R_TRIG;
        }

        // Triangle acts as C-button modifier on D-pad; otherwise D-pad maps to C-Left/Right/Down/Up
        // or JPad when Select is held.
        if (psp_buttons & psp_btn::SELECT) != 0 {
            if (psp_buttons & psp_btn::UP) != 0 {
                n64 |= n64_btn::U_JPAD;
            }
            if (psp_buttons & psp_btn::DOWN) != 0 {
                n64 |= n64_btn::D_JPAD;
            }
            if (psp_buttons & psp_btn::LEFT) != 0 {
                n64 |= n64_btn::L_JPAD;
            }
            if (psp_buttons & psp_btn::RIGHT) != 0 {
                n64 |= n64_btn::R_JPAD;
            }
        } else if (psp_buttons & psp_btn::TRIANGLE) != 0 {
            n64 |= n64_btn::U_CBUTTONS;
            if (psp_buttons & psp_btn::LEFT) != 0 {
                n64 |= n64_btn::L_CBUTTONS;
            }
            if (psp_buttons & psp_btn::RIGHT) != 0 {
                n64 |= n64_btn::R_CBUTTONS;
            }
            if (psp_buttons & psp_btn::DOWN) != 0 {
                n64 |= n64_btn::D_CBUTTONS;
            }
        } else {
            if (psp_buttons & psp_btn::UP) != 0 {
                n64 |= n64_btn::U_CBUTTONS;
            }
            if (psp_buttons & psp_btn::DOWN) != 0 {
                n64 |= n64_btn::D_CBUTTONS;
            }
            if (psp_buttons & psp_btn::LEFT) != 0 {
                n64 |= n64_btn::L_CBUTTONS;
            }
            if (psp_buttons & psp_btn::RIGHT) != 0 {
                n64 |= n64_btn::R_CBUTTONS;
            }
        }

        let stick_x = map_axis(lx as i32 - 128);
        let stick_y = map_axis(128 - ly as i32); // +Y = up on N64

        self.update_n64_raw(n64, stick_x, stick_y)
    }

    pub fn update_n64_raw(&mut self, cur_buttons: u32, stick_x: i8, stick_y: i8) -> PadInput {
        let press_buttons = cur_buttons & !self.prev_buttons;
        let rel_buttons = !cur_buttons & self.prev_buttons;
        self.prev_buttons = cur_buttons;
        self.state = PadInput {
            cur_buttons,
            press_buttons,
            rel_buttons,
            stick_x,
            stick_y,
        };
        self.state
    }
}

fn map_axis(delta: i32) -> i8 {
    if delta.abs() < PadMgr::DEADZONE {
        0
    } else {
        let scaled = (delta * 80) / 128;
        scaled.clamp(-80, 80) as i8
    }
}
