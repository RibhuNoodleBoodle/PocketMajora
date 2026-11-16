#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

pub const SAMPLE_RATE_MM: u32 = 32_000;
pub const SAMPLE_RATE_PSP: u32 = 44_100;
pub const MAX_VOICES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSynthError {
    InvalidPredictor(u8),
    InvalidFrameSize,
}

/// Standard 2nd-order N64 VADPCM predictor pairs (scale 2048).
pub const DEFAULT_VADPCM_BOOK: [[[i16; 8]; 2]; 4] = [
    [[0, 0, 0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0, 0, 0]],
    [
        [0, 1024, 1536, 1792, 1920, 1984, 2016, 2032],
        [0, 0, -256, -384, -448, -480, -496, -504],
    ],
    [
        [0, 1900, 3200, 3700, 3900, 4000, 4050, 4075],
        [0, -900, -1400, -1750, -1900, -1980, -2010, -2030],
    ],
    [
        [0, 1200, 2100, 2600, 2900, 3100, 3200, 3250],
        [0, -400, -800, -1050, -1200, -1300, -1350, -1380],
    ],
];

/// Decode one 9-byte N64 VADPCM frame into 16 PCM16 samples using `state[2]`.
pub fn decode_vadpcm_frame(
    frame: &[u8],
    book: &[[[i16; 8]; 2]],
    state: &mut [i16; 2],
) -> Result<[i16; 16], AudioSynthError> {
    if frame.len() < 9 {
        return Err(AudioSynthError::InvalidFrameSize);
    }
    let header = frame[0];
    let scale_shift = (header >> 4) as u32;
    let pred_idx = header & 0x0F;
    if (pred_idx as usize) >= book.len() {
        return Err(AudioSynthError::InvalidPredictor(pred_idx));
    }
    let preds = &book[pred_idx as usize];
    let scale = 1i32 << scale_shift.min(12);

    let mut out = [0i16; 16];
    let mut prev2 = state[0] as i32;
    let mut prev1 = state[1] as i32;

    for i in 0..16 {
        let byte = frame[1 + (i >> 1)];
        let nib_u = if (i & 1) == 0 {
            (byte >> 4) & 0x0F
        } else {
            byte & 0x0F
        };
        let nib = if nib_u >= 8 {
            (nib_u as i32) - 16
        } else {
            nib_u as i32
        };
        let residual = nib * scale;
        let p0 = preds[0][i & 7] as i32;
        let p1 = preds[1][i & 7] as i32;
        let predicted = ((prev1 * p0) + (prev2 * p1) + 1024) >> 11;
        let sample = (residual + predicted).clamp(i16::MIN as i32, i16::MAX as i32);
        out[i] = sample as i16;
        prev2 = prev1;
        prev1 = sample;
    }

    state[0] = prev2 as i16;
    state[1] = prev1 as i16;
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqId {
    TitleTheme,
    FileSelectFairy,
    ClockTownDay1,
    ClockTownDay2,
    ClockTownDay3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfxId {
    CursorMove,
    Confirm,
    Cancel,
    ClockBell,
    DawnRooster,
    SwordSlash,
}

#[derive(Clone)]
pub struct Voice {
    pub active: bool,
    pub pcm: Vec<i16>,
    pub pos_fp: u64, // 32.32 fixed-point sample cursor
    pub step_fp: u64,
    pub gain_l: f32,
    pub gain_r: f32,
    pub env_gain: f32,
    pub env_decay_per_sample: f32,
    pub looping: bool,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            active: false,
            pcm: Vec::new(),
            pos_fp: 0,
            step_fp: 1u64 << 32,
            gain_l: 1.0,
            gain_r: 1.0,
            env_gain: 1.0,
            env_decay_per_sample: 0.0,
            looping: false,
        }
    }
}

pub struct AudioMixer {
    pub voices: Vec<Voice>,
    reverb_ring: Vec<[i16; 2]>,
    reverb_idx: usize,
    pub sample_ticks: u64,
    pub current_seq: Option<SeqId>,
    seq_step_samples: u32,
}

impl Default for AudioMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioMixer {
    pub fn new() -> Self {
        let mut voices = Vec::with_capacity(MAX_VOICES);
        for _ in 0..MAX_VOICES {
            voices.push(Voice::default());
        }
        Self {
            voices,
            reverb_ring: vec![[0i16; 2]; 1024],
            reverb_idx: 0,
            sample_ticks: 0,
            current_seq: None,
            seq_step_samples: 0,
        }
    }

    pub fn trigger_pcm_voice(
        &mut self,
        pcm: Vec<i16>,
        pitch_ratio: f32,
        gain_l: f32,
        gain_r: f32,
        decay_per_sample: f32,
        looping: bool,
    ) {
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or(0);
        self.voices[idx] = Voice {
            active: !pcm.is_empty(),
            pcm,
            pos_fp: 0,
            step_fp: ((pitch_ratio.max(0.05) as f64) * ((1u64 << 32) as f64)) as u64,
            gain_l,
            gain_r,
            env_gain: 1.0,
            env_decay_per_sample: decay_per_sample,
            looping,
        };
    }

    pub fn trigger_sfx(&mut self, sfx: SfxId) {
        let (freq_hz, decay) = match sfx {
            SfxId::CursorMove => (880.0f32, 0.0005),
            SfxId::Confirm => (1046.5f32, 0.00025),
            SfxId::Cancel => (392.0f32, 0.0004),
            SfxId::ClockBell => (220.0f32, 0.00008),
            SfxId::DawnRooster => (659.25f32, 0.00015),
            SfxId::SwordSlash => (523.25f32, 0.0006),
        };
        let wave = synth_triangle_cycle(freq_hz, SAMPLE_RATE_MM, 9000);
        self.trigger_pcm_voice(wave, 1.0, 0.75, 0.75, decay, true);
    }

    pub fn start_sequence(&mut self, seq: SeqId) {
        self.current_seq = Some(seq);
        self.seq_step_samples = 0;
        let base_hz = match seq {
            SeqId::TitleTheme => 293.66,      // D4
            SeqId::FileSelectFairy => 440.0,  // A4
            SeqId::ClockTownDay1 => 329.63,   // E4
            SeqId::ClockTownDay2 => 349.23,   // F4
            SeqId::ClockTownDay3 => 392.00,   // G4
        };
        let wave = synth_triangle_cycle(base_hz, SAMPLE_RATE_MM, 7000);
        self.trigger_pcm_voice(wave, 1.0, 0.6, 0.6, 0.0, true);
    }

    /// Mix `frames` stereo 32 kHz samples into `out_stereo` (`frames * 2` i16 values).
    /// Returns peak absolute amplitude across the mixed buffer.
    pub fn mix_stereo_32khz(&mut self, out_stereo: &mut [i16]) -> i16 {
        let frames = out_stereo.len() / 2;
        let mut peak: i16 = 0;

        for f in 0..frames {
            let mut acc_l = 0.0f32;
            let mut acc_r = 0.0f32;

            for v in &mut self.voices {
                if !v.active || v.pcm.is_empty() {
                    continue;
                }
                let idx = (v.pos_fp >> 32) as usize;
                if idx >= v.pcm.len() {
                    if v.looping {
                        v.pos_fp %= (v.pcm.len() as u64) << 32;
                    } else {
                        v.active = false;
                        continue;
                    }
                }
                let cur_idx = ((v.pos_fp >> 32) as usize) % v.pcm.len();
                let s = (v.pcm[cur_idx] as f32) * v.env_gain;
                acc_l += s * v.gain_l;
                acc_r += s * v.gain_r;

                v.pos_fp = v.pos_fp.wrapping_add(v.step_fp);
                if v.looping {
                    let limit = (v.pcm.len() as u64) << 32;
                    if v.pos_fp >= limit {
                        v.pos_fp %= limit;
                    }
                } else if ((v.pos_fp >> 32) as usize) >= v.pcm.len() {
                    v.active = false;
                }
                if v.env_decay_per_sample > 0.0 {
                    v.env_gain = (v.env_gain - v.env_decay_per_sample).max(0.0);
                    if v.env_gain <= 0.0005 {
                        v.active = false;
                    }
                }
            }

            let rev = self.reverb_ring[self.reverb_idx];
            let mix_l = (acc_l + (rev[0] as f32) * 0.12).clamp(-32767.0, 32767.0) as i16;
            let mix_r = (acc_r + (rev[1] as f32) * 0.12).clamp(-32767.0, 32767.0) as i16;
            self.reverb_ring[self.reverb_idx] = [mix_l, mix_r];
            self.reverb_idx = (self.reverb_idx + 1) % self.reverb_ring.len();

            out_stereo[f * 2] = mix_l;
            out_stereo[f * 2 + 1] = mix_r;

            peak = peak.max(mix_l.saturating_abs()).max(mix_r.saturating_abs());
            self.sample_ticks += 1;
        }
        peak
    }
}

/// Generate one clean periodic triangle-wave cycle at `freq_hz` for `sample_rate`.
pub fn synth_triangle_cycle(freq_hz: f32, sample_rate: u32, amp: i16) -> Vec<i16> {
    let period = ((sample_rate as f32) / freq_hz.max(20.0)) as usize;
    let period = period.max(8);
    let mut out = Vec::with_capacity(period);
    for i in 0..period {
        let phase = (i as f32) / (period as f32);
        let tri = if phase < 0.25 {
            phase * 4.0
        } else if phase < 0.75 {
            2.0 - phase * 4.0
        } else {
            phase * 4.0 - 4.0
        };
        out.push((tri * (amp as f32)) as i16);
    }
    out
}

/// Linear stereo resampler from 32,000 Hz to 44,100 Hz (exact ratio `441 / 320`,
/// so `1,600` stereo input frames produce `2,205` stereo output frames).
pub fn resample_32k_to_44k1_stereo(in_32k: &[i16], out_44k1: &mut Vec<i16>) {
    let in_frames = in_32k.len() / 2;
    if in_frames == 0 {
        out_44k1.clear();
        return;
    }
    let out_frames = (in_frames * 441) / 320;
    out_44k1.clear();
    out_44k1.reserve(out_frames * 2);

    for i in 0..out_frames {
        let num = i * 320;
        let idx0 = (num / 441).min(in_frames - 1);
        let idx1 = (idx0 + 1).min(in_frames - 1);
        let frac = ((num % 441) as f32) / 441.0;

        let l0 = in_32k[idx0 * 2] as f32;
        let l1 = in_32k[idx1 * 2] as f32;
        let r0 = in_32k[idx0 * 2 + 1] as f32;
        let r1 = in_32k[idx1 * 2 + 1] as f32;

        out_44k1.push((l0 + (l1 - l0) * frac) as i16);
        out_44k1.push((r0 + (r1 - r0) * frac) as i16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vadpcm_9byte_frame_decode_and_invalid_predictor_negative() {
        let frame = [0x21u8, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        let mut state = [0i16; 2];
        let samples = decode_vadpcm_frame(&frame, &DEFAULT_VADPCM_BOOK, &mut state).unwrap();
        assert_eq!(samples.len(), 16);
        assert!(samples.iter().any(|&s| s != 0));

        // Negative: predictor index 9 >= 4
        let bad_pred = [0x29u8, 0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0];
        assert_eq!(
            decode_vadpcm_frame(&bad_pred, &DEFAULT_VADPCM_BOOK, &mut state),
            Err(AudioSynthError::InvalidPredictor(9))
        );

        // Negative: short frame
        assert_eq!(
            decode_vadpcm_frame(&frame[..4], &DEFAULT_VADPCM_BOOK, &mut state),
            Err(AudioSynthError::InvalidFrameSize)
        );
    }

    #[test]
    fn envelope_mixer_rms_energy_and_muted_silence() {
        let mut mixer = AudioMixer::new();
        let mut buf = vec![0i16; 512 * 2];

        // Active non-zero voice
        let wave = synth_triangle_cycle(440.0, SAMPLE_RATE_MM, 10000);
        mixer.trigger_pcm_voice(wave.clone(), 1.0, 1.0, 1.0, 0.0, true);
        let peak = mixer.mix_stereo_32khz(&mut buf);
        assert!(peak > 1000);
        let sq_sum: u64 = buf.iter().map(|&s| (s as i64 * s as i64) as u64).sum();
        let rms = libm_sqrt_u64(sq_sum / (buf.len() as u64));
        assert!(rms > 500, "expected rms > 500, got {rms}");

        // Muted voice (`gain == 0`) produces exact silence when reverb is cleared
        let mut muted_mixer = AudioMixer::new();
        muted_mixer.trigger_pcm_voice(wave, 1.0, 0.0, 0.0, 0.0, true);
        let peak_muted = muted_mixer.mix_stereo_32khz(&mut buf);
        assert_eq!(peak_muted, 0);
        assert!(buf.iter().all(|&s| s == 0));
    }

    #[test]
    fn resampler_32k_to_44k1_exact_frame_count() {
        let in_32k = vec![1200i16; 1600 * 2];
        let mut out_44k1 = Vec::new();
        resample_32k_to_44k1_stereo(&in_32k, &mut out_44k1);
        assert_eq!(out_44k1.len(), 2205 * 2);
        assert!(out_44k1.iter().all(|&s| s == 1200));
    }

    #[test]
    fn sequence_and_sfx_triggers_produce_audio() {
        let mut mixer = AudioMixer::new();
        mixer.start_sequence(SeqId::ClockTownDay1);
        mixer.trigger_sfx(SfxId::ClockBell);
        let mut buf = vec![0i16; 1024 * 2];
        let peak = mixer.mix_stereo_32khz(&mut buf);
        assert!(peak > 1000);
    }

    #[test]
    fn voice_envelope_decay_deactivates_voice() {
        let mut mixer = AudioMixer::new();
        let wave = synth_triangle_cycle(880.0, SAMPLE_RATE_MM, 8000);
        mixer.trigger_pcm_voice(wave, 1.0, 1.0, 1.0, 0.01, true);
        let mut buf = vec![0i16; 256 * 2];
        let _ = mixer.mix_stereo_32khz(&mut buf);
        assert!(!mixer.voices[0].active);
    }

    #[test]
    fn vadpcm_frame_clamps_without_overflow() {
        let max_frame = [0xC1u8, 0x77, 0x77, 0x77, 0x77, 0x77, 0x77, 0x77, 0x77];
        let mut state = [32760i16, 32760i16];
        let samples = decode_vadpcm_frame(&max_frame, &DEFAULT_VADPCM_BOOK, &mut state).unwrap();
        assert_eq!(samples.len(), 16);
    }

    #[test]
    fn resampler_empty_input_clears_output() {
        let mut out = vec![1, 2, 3];
        resample_32k_to_44k1_stereo(&[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn sequence_day_progression_pitches() {
        let mut mixer = AudioMixer::new();
        mixer.start_sequence(SeqId::ClockTownDay1);
        let mut b1 = vec![0i16; 512 * 2];
        assert!(mixer.mix_stereo_32khz(&mut b1) > 0);
        mixer.start_sequence(SeqId::ClockTownDay3);
        assert!(mixer.mix_stereo_32khz(&mut b1) > 0);
    }

    fn libm_sqrt_u64(v: u64) -> u64 {
        let mut x = v;
        let mut y = x.div_ceil(2);
        while y < x {
            x = y;
            y = (x + v / x) / 2;
        }
        x
    }
}
