#![no_std]
#![no_main]

extern crate alloc;

mod platform;

use alloc::format;
use core::sync::atomic::Ordering;
use pmm_assets::ResourceManager;
use pmm_audio::{AudioSynthError, DEFAULT_VADPCM_BOOK, decode_vadpcm_frame};
use pmm_game::graph::{FRAME_40FPS_USEC, NoCatchUpPacer};
use psp::sys;

psp::module!("PocketMajora", 1, 0);

fn psp_main() {
    psp::callback::setup_exit_callback().unwrap();
    let _ = platform::clock::lock_max_clocks();

    let mut pools = platform::memory::PartitionPools::allocate().unwrap();
    let source = platform::pak_io::PspPakSource::open("data/pmm_2s2h.pak").unwrap();
    let mut mgr = ResourceManager::open(source).unwrap();

    // Verify real US Majora's Mask audio segments exist in data/pmm_2s2h.pak
    let has_audiotable = mgr.lookup_name("__OTR__seg/Audiotable").is_some();
    let has_audiobank = mgr.lookup_name("__OTR__seg/Audiobank").is_some();

    let mut power = platform::power::PowerCoordinator::register();
    let mut pacer = NoCatchUpPacer::new();

    let mut audio_worker = platform::audio_psp::AudioWorker::start(
        pools.audio_pool.as_mut_ptr(),
        platform::memory::POOL_AUDIO_BYTES,
    )
    .unwrap();

    // Phase 1: run ~1.0s so >= 10 hardware audio buffers are mixed & submitted
    for _ in 0..40u32 {
        let now = unsafe { sys::sceKernelGetSystemTimeLow() };
        let wait = pacer.compute_sleep_usec(now, FRAME_40FPS_USEC);
        unsafe { sys::sceKernelDelayThread(wait.max(25_000)) };
    }

    let buffers_before_suspend =
        platform::audio_psp::AUDIO_BUFFERS_SUBMITTED.load(Ordering::Acquire);
    let peak_sample = platform::audio_psp::AUDIO_PEAK_SAMPLE.load(Ordering::Acquire);

    // Phase 2: Coordinated suspend -> verify audio paused & pak FD closed
    power.enter_suspend(&mut mgr);
    let suspend_ok =
        platform::audio_psp::AUDIO_PAUSED.load(Ordering::Acquire) && !mgr.is_open();

    // Phase 3: Coordinated resume -> verify pak FD reopened & RTC delta clamped
    let reopen_ok = power.resume_from_suspend(&mut mgr, &mut pacer);
    let resume_ok = reopen_ok && mgr.is_open() && power.rtc_delta_clamped;

    // Phase 4: Run ~0.8s post-resume so audio buffers continue advancing
    for _ in 0..32u32 {
        let now = unsafe { sys::sceKernelGetSystemTimeLow() };
        let wait = pacer.compute_sleep_usec(now, FRAME_40FPS_USEC);
        unsafe { sys::sceKernelDelayThread(wait.max(25_000)) };
    }
    let buffers_after_resume =
        platform::audio_psp::AUDIO_BUFFERS_SUBMITTED.load(Ordering::Acquire);

    audio_worker.stop_and_join();

    // Negative test: invalid VADPCM predictor index
    let bad_frame = [0x29u8, 0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0];
    let mut st = [0i16; 2];
    let neg_pred_ok = matches!(
        decode_vadpcm_frame(&bad_frame, &DEFAULT_VADPCM_BOOK, &mut st),
        Err(AudioSynthError::InvalidPredictor(9))
    );

    let pass = buffers_before_suspend >= 10
        && peak_sample > 1000
        && suspend_ok
        && resume_ok
        && buffers_after_resume > buffers_before_suspend
        && neg_pred_ok;

    let status = if pass { "PASS" } else { "FAIL" };
    let susp_str = if suspend_ok {
        "OK(audio_paused=true, pak_fd_closed=true)"
    } else {
        "FAIL"
    };
    let res_str = if resume_ok {
        "OK(pak_fd_reopened=true, rtc_delta_clamped=true)"
    } else {
        "FAIL"
    };
    let neg_str = if neg_pred_ok {
        "EXPECTED_ERR"
    } else {
        "UNEXPECTED"
    };

    let report = format!(
        "STATUS: {status}\n\
         AUDIO_INIT: OK(rate=32000, channels=2, pool4_bytes={})\n\
         REAL_MM_PAK_AUDIO_SEGS: Audiotable={has_audiotable}, Audiobank={has_audiobank}\n\
         AUDIO_THREAD_BUFFERS_SUBMITTED: {buffers_before_suspend}\n\
         AUDIO_NONZERO_PEAK_SAMPLE: {peak_sample}\n\
         SUSPEND_CYCLE_1_ENTER: {susp_str}\n\
         SUSPEND_CYCLE_1_RESUME: {res_str}\n\
         AUDIO_BUFFERS_AFTER_RESUME: {buffers_after_resume}\n\
         NEGATIVE_INVALID_PREDICTOR_GUARD: {neg_str}\n",
        platform::memory::POOL_AUDIO_BYTES
    );

    let _ = platform::crash::write_text_file("step07_audio_power.log", &report);
    unsafe { sys::sceKernelExitGame() };
}
