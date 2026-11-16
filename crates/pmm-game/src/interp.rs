use alloc::vec;
use alloc::vec::Vec;
use pmm_gfx::math::{Mat4, mat4_identity, mat4_lerp};

const MAX_MATRICES: usize = 2048;

#[derive(Clone, Copy)]
struct MatrixRecord {
    identity: u64,
    site: u32,
    value: Mat4,
    prev_idx: i32,
}

impl Default for MatrixRecord {
    fn default() -> Self {
        Self {
            identity: 0,
            site: 0,
            value: mat4_identity(),
            prev_idx: -1,
        }
    }
}

struct FrameHistory {
    records: Vec<MatrixRecord>,
    valid: bool,
    state_id: u32,
    scene_id: i32,
    room_id: i32,
    camera_epoch: i32,
}

impl FrameHistory {
    fn new() -> Self {
        Self {
            records: Vec::with_capacity(256),
            valid: false,
            state_id: 0,
            scene_id: -1,
            room_id: -1,
            camera_epoch: 0,
        }
    }
}

/// 2S2H-compatible double-buffered matrix history for 40 FPS presentation
/// running over 20 Hz deterministic N64 game logic.
pub struct FrameInterpolator {
    histories: [FrameHistory; 2],
    current: usize,
    recording: bool,
    pub blends_performed: u64,
    pub snaps_performed: u64,
}

impl Default for FrameInterpolator {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameInterpolator {
    pub fn new() -> Self {
        Self {
            histories: [FrameHistory::new(), FrameHistory::new()],
            current: 0,
            recording: false,
            blends_performed: 0,
            snaps_performed: 0,
        }
    }

    pub fn reset(&mut self) {
        self.histories[0].valid = false;
        self.histories[1].valid = false;
        self.recording = false;
    }

    pub fn begin_tick(&mut self, state_id: u32, scene_id: i32, room_id: i32, camera_epoch: i32) {
        self.current ^= 1;
        let cur = &mut self.histories[self.current];
        cur.records.clear();
        cur.valid = false;
        cur.state_id = state_id;
        cur.scene_id = scene_id;
        cur.room_id = room_id;
        cur.camera_epoch = camera_epoch;
        self.recording = true;
    }

    pub fn record(&mut self, identity: u64, site: u32, mtx: &Mat4) {
        if !self.recording {
            return;
        }
        let cur = &mut self.histories[self.current];
        if cur.records.len() >= MAX_MATRICES {
            self.recording = false;
            return;
        }
        if let Some(existing) = cur
            .records
            .iter_mut()
            .find(|r| r.identity == identity && r.site == site)
        {
            existing.value = *mtx;
            return;
        }
        cur.records.push(MatrixRecord {
            identity,
            site,
            value: *mtx,
            prev_idx: -1,
        });
    }

    pub fn end_tick(&mut self) {
        let cur_idx = self.current;
        let prev_idx = self.current ^ 1;
        let valid = self.recording && !self.histories[cur_idx].records.is_empty();
        self.histories[cur_idx].valid = valid;
        self.recording = false;
        if !valid {
            return;
        }

        let compatible = self.histories[prev_idx].valid
            && self.histories[cur_idx].state_id == self.histories[prev_idx].state_id
            && self.histories[cur_idx].scene_id == self.histories[prev_idx].scene_id
            && self.histories[cur_idx].room_id == self.histories[prev_idx].room_id
            && self.histories[cur_idx].camera_epoch == self.histories[prev_idx].camera_epoch;
        if !compatible {
            return;
        }

        let (left, right) = self.histories.split_at_mut(1);
        let (cur_hist, prev_hist) = if cur_idx == 0 {
            (&mut left[0], &right[0])
        } else {
            (&mut right[0], &left[0])
        };

        for rec in &mut cur_hist.records {
            if let Some(p_i) = prev_hist
                .records
                .iter()
                .position(|p| p.identity == rec.identity && p.site == rec.site)
            {
                rec.prev_idx = p_i as i32;
            }
        }
    }

    pub fn resolve(&mut self, identity: u64, site: u32, curr: &Mat4, fraction: f32) -> Mat4 {
        if fraction >= 1.0 {
            return *curr;
        }
        let cur_hist = &self.histories[self.current];
        if !cur_hist.valid {
            self.snaps_performed += 1;
            return *curr;
        }
        let Some(rec) = cur_hist
            .records
            .iter()
            .find(|r| r.identity == identity && r.site == site)
        else {
            self.snaps_performed += 1;
            return *curr;
        };
        if rec.prev_idx < 0 {
            self.snaps_performed += 1;
            return *curr;
        }
        let prev = &self.histories[self.current ^ 1].records[rec.prev_idx as usize].value;
        if !can_blend(prev, curr) {
            self.snaps_performed += 1;
            return *curr;
        }
        self.blends_performed += 1;
        mat4_lerp(prev, curr, fraction)
    }

    pub fn has_sample(&self) -> bool {
        self.histories[self.current].valid
    }
}

/// Reject teleports (> 1000 units), >90 deg basis flips (`dot <= 0`), >4x scale jumps,
/// and non-finite values so skeletons never collapse through singular matrices.
pub fn can_blend(a: &Mat4, b: &Mat4) -> bool {
    for r in 0..4 {
        for c in 0..4 {
            if !a[r][c].is_finite() || !b[r][c].is_finite() {
                return false;
            }
        }
    }
    let mut dist_sq = 0.0f32;
    for c in 0..3 {
        let d = a[3][c] - b[3][c];
        dist_sq += d * d;
    }
    if dist_sq > 1000.0 * 1000.0 {
        return false;
    }
    for r in 0..3 {
        let mut dot = 0.0f32;
        let mut aa = 0.0f32;
        let mut bb = 0.0f32;
        for c in 0..3 {
            dot += a[r][c] * b[r][c];
            aa += a[r][c] * a[r][c];
            bb += b[r][c] * b[r][c];
        }
        if dot <= 0.0 || aa > bb * 16.0 || bb > aa * 16.0 {
            return false;
        }
    }
    true
}

/// Preallocate helper for unused `vec!` warning suppression.
#[allow(dead_code)]
fn _unused() -> Vec<u8> {
    vec![]
}
