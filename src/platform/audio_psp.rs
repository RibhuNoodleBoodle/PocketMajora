use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use pmm_audio::{AudioMixer, SeqId, SfxId, resample_32k_to_44k1_stereo};
use psp::audio::{AudioChannel, AudioFormat, OutputFrequency, SrcChannel};
use psp::sys::{self, ThreadAttributes};

const AUDIO_BLOCK_FRAMES: usize = 1024;

pub static AUDIO_PAUSED: AtomicBool = AtomicBool::new(false);
pub static AUDIO_STOP: AtomicBool = AtomicBool::new(false);
pub static AUDIO_BUFFERS_SUBMITTED: AtomicU32 = AtomicU32::new(0);
pub static AUDIO_PEAK_SAMPLE: AtomicI32 = AtomicI32::new(0);
static AUDIO_SEQ_REQUEST: AtomicI32 = AtomicI32::new(0); // 1 = start ClockTownDay1 + Bell

pub struct AudioWorker {
    thread_id: sys::SceUid,
}

impl AudioWorker {
    /// Start the dedicated PSP 32 kHz audio synthesis thread (`priority = 0x12`).
    /// Uses `SrcChannel(32 kHz)` with automatic fallback to `AudioChannel(44.1 kHz)`.
    pub fn start(pool4_ptr: *mut u8, pool4_bytes: usize) -> Result<Self, i32> {
        let _ = (pool4_ptr, pool4_bytes);
        AUDIO_PAUSED.store(false, Ordering::SeqCst);
        AUDIO_STOP.store(false, Ordering::SeqCst);
        AUDIO_BUFFERS_SUBMITTED.store(0, Ordering::SeqCst);
        AUDIO_PEAK_SAMPLE.store(0, Ordering::SeqCst);
        AUDIO_SEQ_REQUEST.store(1, Ordering::SeqCst);

        let thid = unsafe {
            sys::sceKernelCreateThread(
                b"pmm_audio\0".as_ptr(),
                audio_thread_entry,
                0x12,
                32 * 1024,
                ThreadAttributes::empty(),
                ptr::null_mut(),
            )
        };
        if thid.0 < 0 {
            return Err(thid.0);
        }
        let ret = unsafe { sys::sceKernelStartThread(thid, 0, ptr::null_mut()) };
        if ret < 0 {
            unsafe { sys::sceKernelDeleteThread(thid) };
            return Err(ret);
        }
        Ok(Self { thread_id: thid })
    }

    pub fn trigger_night_bell(&self) {
        AUDIO_SEQ_REQUEST.store(2, Ordering::Release);
    }

    pub fn stop_and_join(&mut self) {
        AUDIO_STOP.store(true, Ordering::SeqCst);
        unsafe {
            sys::sceKernelWaitThreadEnd(self.thread_id, ptr::null_mut());
            sys::sceKernelDeleteThread(self.thread_id);
        }
    }
}

unsafe extern "C" fn audio_thread_entry(_args: usize, _argp: *mut c_void) -> i32 {
    let mut mixer = AudioMixer::new();
    let mut buf_32k = vec![0i16; AUDIO_BLOCK_FRAMES * 2];
    let mut buf_44k = Vec::with_capacity(3072 * 2);

    enum OutCh {
        Src(SrcChannel),
        Pcm(AudioChannel),
    }

    let out_ch = match SrcChannel::reserve(AUDIO_BLOCK_FRAMES as i32, OutputFrequency::Khz32) {
        Ok(s) => OutCh::Src(s),
        Err(_) => {
            let pcm_frames = ((AUDIO_BLOCK_FRAMES * 441) / 320 + 63) & !63;
            match AudioChannel::reserve(pcm_frames as i32, AudioFormat::Stereo) {
                Ok(c) => OutCh::Pcm(c),
                Err(_) => return -1,
            }
        }
    };

    while !AUDIO_STOP.load(Ordering::Acquire) {
        match AUDIO_SEQ_REQUEST.swap(0, Ordering::AcqRel) {
            1 => {
                mixer.start_sequence(SeqId::ClockTownDay1);
                mixer.trigger_sfx(SfxId::ClockBell);
            }
            2 => {
                mixer.trigger_sfx(SfxId::ClockBell);
            }
            _ => {}
        }
        if AUDIO_PAUSED.load(Ordering::Acquire) {
            unsafe { sys::sceKernelDelayThread(4_000) };
            continue;
        }

        let peak = mixer.mix_stereo_32khz(&mut buf_32k);
        if (peak as i32) > AUDIO_PEAK_SAMPLE.load(Ordering::Relaxed) {
            AUDIO_PEAK_SAMPLE.store(peak as i32, Ordering::Relaxed);
        }

        let res = match &out_ch {
            OutCh::Src(src) => src.output_blocking(0x8000, &buf_32k),
            OutCh::Pcm(ch) => {
                resample_32k_to_44k1_stereo(&buf_32k, &mut buf_44k);
                let needed = (ch.sample_count() as usize) * 2;
                if buf_44k.len() < needed {
                    buf_44k.resize(needed, 0);
                }
                ch.output_blocking(0x8000, &buf_44k)
            }
        };
        if res.is_ok() {
            AUDIO_BUFFERS_SUBMITTED.fetch_add(1, Ordering::Relaxed);
        } else {
            unsafe { sys::sceKernelDelayThread(2_000) };
        }
    }
    0
}
