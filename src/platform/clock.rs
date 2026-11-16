use psp::sys;

/// Lock PSP-3000 hardware clocks to maximum (333 MHz CPU / 333 MHz PLL / 166 MHz Bus)
/// for the 40 FPS / 25 ms latency target. Returns `(cpu_mhz, bus_mhz)` read back
/// from firmware after locking.
pub fn lock_max_clocks() -> Result<(i32, i32), i32> {
    let ret = unsafe { sys::scePowerSetClockFrequency(333, 333, 166) };
    if ret < 0 {
        return Err(ret);
    }
    let cpu = unsafe { sys::scePowerGetCpuClockFrequency() };
    let bus = unsafe { sys::scePowerGetBusClockFrequency() };
    Ok((cpu, bus))
}
