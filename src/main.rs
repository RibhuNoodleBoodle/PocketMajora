#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use psp::sys;

psp::module!("PocketMajora", 1, 0);

fn psp_main() {
    psp::callback::setup_exit_callback().unwrap();

    // Step 02: Lock PSP-3000 clocks to 333 MHz CPU / 166 MHz Bus for 40 FPS target.
    let (cpu_mhz, bus_mhz) = platform::clock::lock_max_clocks().unwrap_or((0, 0));

    // Allocate the 4 fixed Partition 2 memory pools (28 MiB total).
    let pools_res = platform::memory::PartitionPools::allocate();
    let report = match pools_res {
        Ok(mut pools) => {
            let info = pools.info();
            let free_after = unsafe { sys::sceKernelTotalFreeMemSize() };
            let neg_res = platform::memory::alloc_oversize_negative_test();

            // Run 100 scene + room mark/reset cycles on Pool 2 and check zero leak.
            let mut arena = pools.scene_arena_mut();
            let used_before = arena.used_bytes();
            let mut alloc_ok = true;
            for _ in 0..100u32 {
                arena.mark_scene();
                if arena.alloc_aligned(131_072, 64).is_none() {
                    alloc_ok = false;
                }
                for _ in 0..2u32 {
                    arena.mark_room();
                    if arena.alloc_aligned(65_536, 16).is_none() {
                        alloc_ok = false;
                    }
                    arena.reset_to_room();
                }
                arena.reset_to_scene();
            }
            let used_after = arena.used_bytes();
            let leak_bytes = used_after.saturating_sub(used_before);

            let neg_str = match neg_res {
                Ok(()) => format!("UNEXPECTED_OK"),
                Err(code) => format!("EXPECTED_ERR({:#010x})", code as u32),
            };
            let all_aligned = info.iter().all(|p| p.align_64);
            let pass = cpu_mhz == 333
                && bus_mhz == 166
                && all_aligned
                && free_after >= 10 * 1024 * 1024
                && neg_res.is_err()
                && alloc_ok
                && leak_bytes == 0;

            let status = if pass { "PASS" } else { "FAIL" };
            let schema_magic_ok =
                pmm_schema::PmmPakHeader::new(0, 64, 64, 0, 0, 64).magic == pmm_schema::PAK_MAGIC;

            format!(
                "STATUS: {status}\n\
                 CPU_BUS_CLOCK_MHZ: {cpu_mhz}/{bus_mhz}\n\
                 POOL_COMMON_6MIB: OK(addr={:#010x}, align_64={})\n\
                 POOL_SCENE_8MIB: OK(addr={:#010x}, align_64={})\n\
                 POOL_RENDERER_6MIB: OK(addr={:#010x}, align_64={})\n\
                 POOL_AUDIO_8MIB: OK(addr={:#010x}, align_64={})\n\
                 FREE_AFTER_28MIB_POOLS: {free_after}\n\
                 NEGATIVE_OVERSIZE_48MIB_ALLOC: {neg_str}\n\
                 ARENA_100_RESET_CYCLES_LEAK_BYTES: {leak_bytes}\n\
                 PMM_SCHEMA_MAGIC_CHECK: {schema_magic_ok}\n",
                info[0].addr,
                info[0].align_64,
                info[1].addr,
                info[1].align_64,
                info[2].addr,
                info[2].align_64,
                info[3].addr,
                info[3].align_64,
            )
        }
        Err(code) => format!("STATUS: FAIL_POOL_ALLOC({:#010x})\n", code as u32),
    };

    let _ = platform::crash::write_text_file("step02_probe.log", &report);

    unsafe {
        sys::sceKernelExitGame();
    }
}
