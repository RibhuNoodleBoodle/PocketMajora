use crate::save::SaveContext;

pub const CLOCK_DAWN: u16 = 0x4000; // 6:00 AM
pub const CLOCK_NIGHT: u16 = 0xC000; // 6:00 PM
pub const TIME_SPEED_NORMAL: i32 = 30;
pub const TIME_SPEED_INVERTED: i32 = 10; // Exact 1/3 rate under Inverted Song of Time

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockEvent {
    None,
    NightStarted(i32),
    DawnTransition(i32),
    MoonCrash,
}

/// Authentic N64 Majora's Mask 3-Day Clock controller (`time: u16`, `day: 1..=3`, `R_TIME_SPEED`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreeDayClock {
    pub day: i32,
    pub time: u16,
    pub r_time_speed: i32,
    pub inverted_song_active: bool,
    pub paused: bool,
}

impl Default for ThreeDayClock {
    fn default() -> Self {
        Self::new(1, CLOCK_DAWN)
    }
}

impl ThreeDayClock {
    pub fn new(day: i32, time: u16) -> Self {
        Self {
            day: day.clamp(1, 3),
            time,
            r_time_speed: TIME_SPEED_NORMAL,
            inverted_song_active: false,
            paused: false,
        }
    }

    pub fn set_inverted_song(&mut self, active: bool) {
        self.inverted_song_active = active;
        self.r_time_speed = if active {
            TIME_SPEED_INVERTED
        } else {
            TIME_SPEED_NORMAL
        };
    }

    pub fn effective_speed(&self) -> i32 {
        if self.paused {
            0
        } else {
            self.r_time_speed
        }
    }

    /// True during final 6 hours of Day 3 (`day == 3 && time >= 0x0000 && time < 0x4000`).
    #[inline]
    pub fn is_final_hours(&self) -> bool {
        self.day == 3 && self.time < CLOCK_DAWN
    }

    /// Advance clock by one 20 Hz game tick (using current `r_time_speed`).
    pub fn tick(&mut self) -> ClockEvent {
        let speed = self.effective_speed();
        if speed <= 0 {
            return ClockEvent::None;
        }
        self.advance_units(speed as u16)
    }

    /// Advance by an arbitrary delta (<= 0x8000) detecting `0xC000` (6 PM) and `0x4000` (6 AM) boundaries.
    pub fn advance_units(&mut self, delta: u16) -> ClockEvent {
        if self.day > 3 {
            return ClockEvent::MoonCrash;
        }
        let prev = self.time;
        let next = prev.wrapping_add(delta);
        self.time = next;

        // Check crossing of 0x4000 (Dawn / new day) — happens at day end (post-midnight)
        let crossed_dawn = if prev < next {
            prev < CLOCK_DAWN && next >= CLOCK_DAWN
        } else {
            // Wrapped past 0xFFFF -> 0x0000: hit 0x4000 if next >= 0x4000
            next >= CLOCK_DAWN || prev < CLOCK_DAWN
        };

        if crossed_dawn {
            self.day += 1;
            if self.day > 3 {
                self.time = CLOCK_DAWN;
                return ClockEvent::MoonCrash;
            }
            return ClockEvent::DawnTransition(self.day);
        }

        // Check crossing of 0xC000 (6:00 PM Nightfall)
        let crossed_night = if prev < next {
            prev < CLOCK_NIGHT && next >= CLOCK_NIGHT
        } else {
            next >= CLOCK_NIGHT && prev < CLOCK_NIGHT
        };

        if crossed_night {
            return ClockEvent::NightStarted(self.day);
        }

        ClockEvent::None
    }

    /// Song of Double Time: jumps cleanly to the next `0x4000` or `0xC000` boundary
    /// without skipping past Final Day 6:00 AM.
    pub fn song_of_double_time(&mut self) -> ClockEvent {
        if self.time >= CLOCK_DAWN && self.time < CLOCK_NIGHT {
            self.time = CLOCK_NIGHT;
            ClockEvent::NightStarted(self.day)
        } else if self.day >= 3 && self.time < CLOCK_DAWN {
            // Final hours of Day 3: cannot skip past Final Day 6:00 AM into Day 4 via Double Time.
            self.time = CLOCK_DAWN - 1;
            ClockEvent::None
        } else {
            self.time = CLOCK_DAWN;
            self.day += 1;
            ClockEvent::DawnTransition(self.day)
        }
    }

    /// Song of Time: reset to Dawn of the First Day (`day = 1, time = 0x4000`),
    /// increments `three_day_reset_count`, preserves masks/banked rupees, resets consumables.
    pub fn song_of_time_reset(&mut self, save: &mut SaveContext) {
        self.day = 1;
        self.time = CLOCK_DAWN;
        self.set_inverted_song(false);
        save.three_day_reset_count = save.three_day_reset_count.saturating_add(1);
        save.day = 1;
        save.time = CLOCK_DAWN;
        save.total_days = 0;
        // Field consumables reset while permanent bank & mask state stay preserved:
        save.rupees = 0;
        save.magic = 0;
        save.health = save.health_capacity;
    }

    pub fn sync_to_save(&self, save: &mut SaveContext) {
        save.day = self.day;
        save.time = self.time;
    }
}
