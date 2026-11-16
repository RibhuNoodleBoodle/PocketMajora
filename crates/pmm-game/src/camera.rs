use pmm_gfx::math::{Mat4, RectI32, compute_letterbox_scissor, mat4_identity};

use crate::interp::FrameInterpolator;

pub const CAMERA_ENTITY_ID: u64 = 0x4341_4D30;
pub const Z_TARGET_LETTERBOX_MAX_PX: u16 = 24;

pub struct Camera {
    pub eye: [f32; 3],
    pub at: [f32; 3],
    pub letterbox_bar_px: u16,
    pub scissor: (RectI32, RectI32, RectI32),
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera {
    pub fn new() -> Self {
        Self {
            eye: [0.0, 90.0, 200.0],
            at: [0.0, 32.0, 0.0],
            letterbox_bar_px: 0,
            scissor: compute_letterbox_scissor(480, 272, 0),
        }
    }

    pub fn letterbox_gap_rows(&self) -> i32 {
        let (scene, top, bot) = self.scissor;
        (scene.y0 - top.y1).abs() + (bot.y0 - scene.y1).abs()
    }

    /// 20 Hz camera follow & Z-target letterbox (`0..=24` px) update.
    pub fn update_logic(
        &mut self,
        player_pos: [f32; 3],
        player_yaw: f32,
        z_target_active: bool,
        interp: &mut FrameInterpolator,
    ) {
        if z_target_active {
            self.letterbox_bar_px = (self.letterbox_bar_px + 6).min(Z_TARGET_LETTERBOX_MAX_PX);
        } else {
            self.letterbox_bar_px = self.letterbox_bar_px.saturating_sub(6);
        }
        self.scissor = compute_letterbox_scissor(480, 272, self.letterbox_bar_px as i32);

        let dist = if z_target_active { 120.0 } else { 180.0 };
        let back_x = -libm::sinf(player_yaw) * dist;
        let back_z = -libm::cosf(player_yaw) * dist;
        self.at = [player_pos[0], player_pos[1] + 36.0, player_pos[2]];
        self.eye = [
            player_pos[0] + back_x,
            player_pos[1] + if z_target_active { 78.0 } else { 96.0 },
            player_pos[2] + back_z,
        ];

        let mut mtx = mat4_identity();
        mtx[3][0] = self.eye[0];
        mtx[3][1] = self.eye[1];
        mtx[3][2] = self.eye[2];
        interp.record(CAMERA_ENTITY_ID, 0, &mtx);
    }

    pub fn build_view_matrix(&self, interp: &mut FrameInterpolator, fraction: f32) -> Mat4 {
        let mut target = mat4_identity();
        target[3][0] = self.eye[0];
        target[3][1] = self.eye[1];
        target[3][2] = self.eye[2];
        let lerped_eye = interp.resolve(CAMERA_ENTITY_ID, 0, &target, fraction);
        let mut view = mat4_identity();
        view[3][0] = -lerped_eye[3][0] * 0.01;
        view[3][1] = -lerped_eye[3][1] * 0.01;
        view[3][2] = -lerped_eye[3][2] * 0.01;
        view
    }
}
