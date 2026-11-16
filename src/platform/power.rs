use crate::platform::audio_psp::AUDIO_PAUSED;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
use pmm_assets::{PakSource, ResourceManager};
use pmm_game::graph::NoCatchUpPacer;
use psp::power::{PowerCallbackHandle, on_power_event};
use psp::sys::{self, GuSyncBehavior, GuSyncMode, PowerInfo};

pub static SUSPEND_REQUESTED: AtomicBool = AtomicBool::new(false);
pub static RESUME_REQUESTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn power_cb(_unknown: i32, power_info: i32, _common: *mut c_void) -> i32 {
    let flags = PowerInfo::from_bits_truncate(power_info as u32);
    if flags.contains(PowerInfo::SUSPENDING) || flags.contains(PowerInfo::STANDBY) {
        AUDIO_PAUSED.store(true, Ordering::SeqCst);
        SUSPEND_REQUESTED.store(true, Ordering::SeqCst);
    }
    if flags.contains(PowerInfo::RESUME_COMPLETE) {
        RESUME_REQUESTED.store(true, Ordering::SeqCst);
    }
    0
}

pub struct PowerCoordinator {
    _cb_handle: Option<PowerCallbackHandle>,
    pub suspended: bool,
    pub rtc_delta_clamped: bool,
}

impl PowerCoordinator {
    pub fn register() -> Self {
        let handle = on_power_event(power_cb).ok();
        Self {
            _cb_handle: handle,
            suspended: false,
            rtc_delta_clamped: false,
        }
    }

    pub fn enter_suspend<S: PakSource>(&mut self, mgr: &mut ResourceManager<S>) {
        AUDIO_PAUSED.store(true, Ordering::SeqCst);
        unsafe {
            sys::sceKernelDelayThread(25_000);
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
        }
        mgr.suspend_close();
        self.suspended = true;
    }

    pub fn resume_from_suspend<S: PakSource>(
        &mut self,
        mgr: &mut ResourceManager<S>,
        pacer: &mut NoCatchUpPacer,
    ) -> bool {
        let reopen_ok = mgr.resume_reopen().is_ok();
        pacer.reset(); // Clamp RTC delta so 3-Day Clock does not leap forward by sleep duration
        self.rtc_delta_clamped = true;
        self.suspended = false;
        AUDIO_PAUSED.store(false, Ordering::SeqCst);
        reopen_ok
    }
}
