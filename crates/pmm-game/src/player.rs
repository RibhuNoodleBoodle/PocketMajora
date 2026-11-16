use pmm_gfx::math::mat4_identity;

use crate::bgcheck::{BGCHECK_Y_MIN, BgCheck, CollisionMesh};
use crate::interp::FrameInterpolator;
use crate::padmgr::{PadInput, n64_btn};

pub const PLAYER_ENTITY_ID: u64 = 0x504C_4159;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerForm {
    Human,
    DekuScrub,
    Zora,
    Goron,
}

impl PlayerForm {
    pub fn name(self) -> &'static str {
        match self {
            Self::Human => "Human",
            Self::DekuScrub => "Deku",
            Self::Zora => "Zora",
            Self::Goron => "Goron",
        }
    }

    pub fn move_speed(self) -> f32 {
        match self {
            Self::Human => 8.5,
            Self::DekuScrub => 7.0,
            Self::Zora => 9.5,
            Self::Goron => 12.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerAction {
    Idle,
    Moving,
    Jumping,
    Rolling,
    SwordSlash,
    Interact,
    Ocarina,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcarinaSong {
    None,
    SongOfTime,
    InvertedSongOfTime,
    SongOfDoubleTime,
}

pub struct Player {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub form: PlayerForm,
    pub action: PlayerAction,
    pub z_target_active: bool,
    pub grounded: bool,
    pub floor_y: f32,
    pub wall_hits_total: u32,
    pub exit_triggered: Option<u8>,
    pub total_distance_moved: f32,
    pub sword_timer: u8,
    pub roll_timer: u8,
    pub transform_count: u32,
}

impl Player {
    pub fn new(spawn: [f32; 3], form: PlayerForm) -> Self {
        Self {
            pos: spawn,
            vel: [0.0; 3],
            yaw: 0.0,
            form,
            action: PlayerAction::Idle,
            z_target_active: false,
            grounded: true,
            floor_y: spawn[1],
            wall_hits_total: 0,
            exit_triggered: None,
            total_distance_moved: 0.0,
            sword_timer: 0,
            roll_timer: 0,
            transform_count: 0,
        }
    }

    /// 20 Hz Player simulation tick: movement, jump/roll, Deku/Human toggle (`C_RIGHT`),
    /// Z-targeting (`Z_TRIG`), sword slash (`B_BUTTON`), and `BgCheck` collision.
    pub fn update_logic(
        &mut self,
        pad: &PadInput,
        mesh: &CollisionMesh,
        interp: &mut FrameInterpolator,
    ) {
        self.z_target_active = pad.held(n64_btn::Z_TRIG);

        // Deku / Human mask toggle via C-Right
        if pad.pressed(n64_btn::R_CBUTTONS) {
            self.form = if self.form == PlayerForm::DekuScrub {
                PlayerForm::Human
            } else {
                PlayerForm::DekuScrub
            };
            self.transform_count += 1;
        }

        // Sword slash / roll / action
        if self.sword_timer > 0 {
            self.sword_timer -= 1;
        }
        if self.roll_timer > 0 {
            self.roll_timer -= 1;
        }

        if pad.pressed(n64_btn::B_BUTTON) {
            self.sword_timer = 6;
            self.action = PlayerAction::SwordSlash;
        } else if pad.pressed(n64_btn::A_BUTTON) && libm::hypotf(self.vel[0], self.vel[2]) > 4.0 {
            self.roll_timer = 8;
            self.action = PlayerAction::Rolling;
        } else if pad.pressed(n64_btn::A_BUTTON) && self.grounded {
            self.action = PlayerAction::Interact;
        } else {
            self.action = PlayerAction::Idle;
        }

        // Analog stick (-80..80) -> horizontal displacement
        let sx = (pad.stick_x as f32) / 80.0;
        let sy = (pad.stick_y as f32) / 80.0;
        let speed = self.form.move_speed() * if self.roll_timer > 0 { 1.45 } else { 1.0 };

        let prev_x = self.pos[0];
        let prev_z = self.pos[2];

        self.vel[0] = sx.clamp(-1.0, 1.0) * speed;
        self.vel[2] = (-sy).clamp(-1.0, 1.0) * speed;
        if self.vel[0].abs() > 0.2 || self.vel[2].abs() > 0.2 {
            self.yaw = libm::atan2f(self.vel[0], self.vel[2]);
            if self.sword_timer == 0 && self.roll_timer == 0 {
                self.action = PlayerAction::Moving;
            }
        }

        self.pos[0] += self.vel[0];
        self.pos[2] += self.vel[2];

        // Gravity & Floor raycast
        if !self.grounded {
            self.vel[1] = (self.vel[1] - 2.0).max(-20.0);
            self.pos[1] += self.vel[1];
        }

        // Wall sphere collision
        let hits = BgCheck::entity_sph_vs_wall(mesh, &mut self.pos, 18.0, 44.0);
        self.wall_hits_total = self.wall_hits_total.saturating_add(hits);

        // Floor snap & exit polygon check
        let (fy, exit_idx) = BgCheck::entity_raycast_floor(mesh, self.pos[0], self.pos[1] + 24.0, self.pos[2]);
        if fy > BGCHECK_Y_MIN + 1.0 {
            self.floor_y = fy;
            if self.pos[1] <= fy + 16.0 {
                self.pos[1] = fy;
                self.vel[1] = 0.0;
                self.grounded = true;
            } else {
                self.grounded = false;
            }
        } else {
            self.grounded = false;
        }
        self.exit_triggered = exit_idx;

        let dx = self.pos[0] - prev_x;
        let dz = self.pos[2] - prev_z;
        self.total_distance_moved += libm::sqrtf(dx * dx + dz * dz);

        let mut mtx = mat4_identity();
        mtx[3][0] = self.pos[0];
        mtx[3][1] = self.pos[1];
        mtx[3][2] = self.pos[2];
        interp.record(PLAYER_ENTITY_ID, 0, &mtx);
    }
}
