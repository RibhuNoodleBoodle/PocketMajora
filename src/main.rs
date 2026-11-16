#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use pmm_assets::{ResourceError, ResourceManager, SegmentTable};
use pmm_schema::{PmmPakEntry, crc32_ieee};
use psp::sys;

psp::module!("PocketMajora", 1, 0);

fn psp_main() {
    psp::callback::setup_exit_callback().unwrap();
    let _ = platform::clock::lock_max_clocks();

    let mut pools = match platform::memory::PartitionPools::allocate() {
        Ok(p) => p,
        Err(code) => {
            let msg = format!("STATUS: FAIL_POOL_ALLOC({:#010x})\n", code as u32);
            let _ = platform::crash::write_text_file("step04_assets.log", &msg);
            unsafe { sys::sceKernelExitGame() };
            return;
        }
    };

    let source = match platform::pak_io::PspPakSource::open("data/pmm_2s2h.pak") {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("STATUS: FAIL_PAK_OPEN({e})\n");
            let _ = platform::crash::write_text_file("step04_assets.log", &msg);
            unsafe { sys::sceKernelExitGame() };
            return;
        }
    };

    let mut mgr = match ResourceManager::open(source) {
        Ok(m) => m,
        Err(e) => {
            let msg = format!("STATUS: FAIL_PAK_PARSE({e})\n");
            let _ = platform::crash::write_text_file("step04_assets.log", &msg);
            unsafe { sys::sceKernelExitGame() };
            return;
        }
    };

    let entry_count = mgr.header().entry_count;
    let pak_version = mgr.header().version;

    // Pin common assets into Pool 1 (`CommonAssets`)
    let mut common_arena = pools.common_arena_mut();
    let pinned_ids: alloc::vec::Vec<_> = mgr
        .entries()
        .iter()
        .filter(|e| (e.flags & PmmPakEntry::FLAG_PINNED_COMMON) != 0)
        .map(|e| e.id)
        .collect();
    let mut common_ok = true;
    for id in pinned_ids {
        if mgr.load_into_arena(pmm_schema::ResourceId(id), &mut common_arena).is_err() {
            common_ok = false;
        }
    }
    let common_pinned_bytes = common_arena.used_bytes();

    // 20 consecutive Scene + Room reload cycles inside Pool 2 (`SceneObjectArena`)
    let mut scene_arena = pools.scene_arena_mut();
    let pool2_before = scene_arena.used_bytes();
    let kernel_free_before = unsafe { sys::sceKernelTotalFreeMemSize() };

    let mut cycles_completed = 0u32;
    let mut all_aligned = true;
    let mut crc_mismatch = false;
    let mut seg_check_ok = true;

    for cycle in 0..20u32 {
        scene_arena.mark_scene();

        let scene_info = mgr
            .load_by_name_into_arena("__OTR__scenes/nonmq/Z2_TOWN/Z2_TOWN", &mut scene_arena)
            .ok()
            .map(|(_, s_buf)| (s_buf.as_ptr(), s_buf.len()));
        let vtx_info = mgr
            .load_by_name_into_arena("__OTR__scenes/test/room0_vtx", &mut scene_arena)
            .ok()
            .map(|(_, v_buf)| {
                if cycle == 0 {
                    let _ = crc32_ieee(v_buf);
                }
                (v_buf.as_ptr(), v_buf.len())
            });

        if let (Some((s_ptr, s_len)), Some((v_ptr, v_len))) = (scene_info, vtx_info) {
            if (s_ptr as usize) % 16 != 0 || (v_ptr as usize) % 16 != 0 {
                all_aligned = false;
            }
            let mut segs = SegmentTable::default();
            // SAFETY: Pointers remain valid inside `scene_arena` until `reset_to_scene()`.
            unsafe {
                segs.set_segment(0x02, core::slice::from_raw_parts(s_ptr, s_len));
                segs.set_segment(0x06, core::slice::from_raw_parts(v_ptr, v_len));
            }
            if segs.resolve(0x0600_0000).is_err() || segs.resolve(0x0200_0000).is_err() {
                seg_check_ok = false;
            }
        } else {
            crc_mismatch = true;
        }

        scene_arena.mark_room();
        let room_info = mgr
            .load_by_name_into_arena(
                "__OTR__scenes/nonmq/Z2_TOWN/Z2_TOWN_room_0",
                &mut scene_arena,
            )
            .ok()
            .map(|(_, r_buf)| r_buf.as_ptr() as usize);
        let dl_info = mgr
            .load_by_name_into_arena("__OTR__scenes/test/room0_dl", &mut scene_arena)
            .ok()
            .map(|(_, dl_buf)| dl_buf.as_ptr() as usize);
        if let (Some(r_addr), Some(dl_addr)) = (room_info, dl_info) {
            if r_addr % 16 != 0 || dl_addr % 16 != 0 {
                all_aligned = false;
            }
        } else {
            crc_mismatch = true;
        }
        scene_arena.reset_to_room();
        scene_arena.reset_to_scene();
        cycles_completed += 1;
    }

    let pool2_after = scene_arena.used_bytes();
    let kernel_free_after = unsafe { sys::sceKernelTotalFreeMemSize() };
    let kernel_delta = kernel_free_before - kernel_free_after;

    // Test suspend/resume FD lifecycle
    mgr.suspend_close();
    let closed_err = matches!(
        mgr.load_by_name_into_arena("__OTR__scenes/test/room0_dl", &mut scene_arena),
        Err(ResourceError::FileClosed)
    );
    let reopen_ok = mgr.resume_reopen().is_ok();
    scene_arena.mark_scene();
    let reread_ok = mgr
        .load_by_name_into_arena("__OTR__scenes/test/room0_dl", &mut scene_arena)
        .is_ok();
    scene_arena.reset_to_scene();

    // Negative test: missing resource lookup
    let missing_err = matches!(
        mgr.load_by_name_into_arena("__OTR__does/not/exist", &mut scene_arena),
        Err(ResourceError::NotFound)
    );

    let pass = common_ok
        && !crc_mismatch
        && all_aligned
        && seg_check_ok
        && cycles_completed == 20
        && pool2_before == 0
        && pool2_after == 0
        && kernel_delta == 0
        && closed_err
        && reopen_ok
        && reread_ok
        && missing_err;

    let status = if pass { "PASS" } else { "FAIL" };
    let suspend_str = if closed_err && reopen_ok && reread_ok {
        "OK"
    } else {
        "FAIL"
    };
    let missing_str = if missing_err {
        "EXPECTED_ERR_NOT_FOUND"
    } else {
        "UNEXPECTED"
    };

    let report = format!(
        "STATUS: {status}\n\
         PAK_OPEN: OK(entries={entry_count}, version={pak_version})\n\
         COMMON_PINNED_BYTES: {common_pinned_bytes}\n\
         RELOAD_CYCLES_COMPLETED: {cycles_completed}\n\
         POOL2_USED_BEFORE_CYCLES: {pool2_before}\n\
         POOL2_USED_AFTER_20_CYCLES: {pool2_after}\n\
         KERNEL_FREE_MEM_DELTA_AFTER_20_CYCLES: {kernel_delta}\n\
         PAYLOAD_16B_ALIGNED: {all_aligned}\n\
         SEGMENT_TABLE_CHECK: {seg_check_ok}\n\
         SUSPEND_RESUME_FD_REOPEN_READ: {suspend_str}\n\
         NEGATIVE_MISSING_RESOURCE_LOOKUP: {missing_str}\n"
    );

    let _ = platform::crash::write_text_file("step04_assets.log", &report);
    unsafe { sys::sceKernelExitGame() };
}
