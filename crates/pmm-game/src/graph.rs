pub const FRAME_40FPS_USEC: u32 = 25_000;
pub const TICK_20HZ_USEC: u32 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GameStateId {
    TitleSetup = 0,
    Title = 1,
    FileChoose = 2,
    Opening = 3,
    DayTelop = 4,
    Play = 5,
}

impl GameStateId {
    pub fn name(self) -> &'static str {
        match self {
            Self::TitleSetup => "TitleSetup",
            Self::Title => "Title",
            Self::FileChoose => "FileChoose",
            Self::Opening => "Opening",
            Self::DayTelop => "DayTelop",
            Self::Play => "Play",
        }
    }
}

/// Convert PSP 1 MHz microseconds to N64 46.875 MHz `OS_CPU_COUNTER` units.
#[inline]
pub const fn us_to_n64_cpu_counter(us: u64) -> u64 {
    (us.saturating_mul(46_875)) / 1000
}

/// No-catch-up 40 FPS (`25,000 us` per presented frame) pacer using wrap-safe
/// 32-bit microsecond subtraction (`sceKernelGetSystemTimeLow`).
pub struct NoCatchUpPacer {
    next_deadline_usec: u32,
    initialized: bool,
}

impl Default for NoCatchUpPacer {
    fn default() -> Self {
        Self::new()
    }
}

impl NoCatchUpPacer {
    pub const fn new() -> Self {
        Self {
            next_deadline_usec: 0,
            initialized: false,
        }
    }

    pub fn reset(&mut self) {
        self.initialized = false;
        self.next_deadline_usec = 0;
    }

    /// Compute how many microseconds to sleep before the next 40 FPS presentation pass.
    /// If `now_usec` is past the deadline, rebases immediately (NO catch-up spiral).
    pub fn compute_sleep_usec(&mut self, now_usec: u32, frame_interval_usec: u32) -> u32 {
        if !self.initialized {
            self.next_deadline_usec = now_usec.wrapping_add(frame_interval_usec);
            self.initialized = true;
            return 0;
        }
        let wait = (self.next_deadline_usec.wrapping_sub(now_usec)) as i32;
        if wait > 0 {
            self.next_deadline_usec = self.next_deadline_usec.wrapping_add(frame_interval_usec);
            wait as u32
        } else {
            // Missed deadline: discard debt and rebase from `now_usec`.
            self.next_deadline_usec = now_usec.wrapping_add(frame_interval_usec);
            0
        }
    }
}
