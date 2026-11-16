use pmm_assets::{BoundedArena, PakSource, ResourceManager};
use pmm_gfx::math::mat4_identity;
use pmm_gfx::{GfxBackend, Renderer};

use crate::actors::{ActorStepOutcome, update_actors};
use crate::camera::Camera;
use crate::clock::{ClockEvent, ThreeDayClock};
use crate::graph::GameStateId;
use crate::hud::HudRenderer;
use crate::interp::FrameInterpolator;
use crate::padmgr::{PadInput, n64_btn};
use crate::player::{OcarinaSong, PLAYER_ENTITY_ID, Player, PlayerForm};
use crate::save::{FlashIo, SaveContext, SysFlashrom};
use crate::scene::{ActiveScene, SceneId, otr_play_spawn_scene};
use crate::states::render_f3dex2_quad_layer;
use crate::thga::GfxPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayStepEvent {
    pub clock_event: ClockEvent,
    pub scene_transition: Option<SceneId>,
    pub owl_saved: bool,
    pub play_night_bell_sfx: bool,
}

pub struct PlayState {
    pub scene: ActiveScene,
    pub player: Player,
    pub camera: Camera,
    pub clock: ThreeDayClock,
    pub save: SaveContext,
    pub logic_ticks: u32,
    pub camera_epoch: i32,
    pub owl_save_count: u32,
    pub scene_transition_count: u32,
    pub max_letterbox_bar_px: u16,
    pub scene_leak_bytes_max: isize,
    baseline_scene_arena_used: Option<usize>,
}

impl PlayState {
    pub fn enter<S: PakSource>(
        initial_scene: SceneId,
        save: SaveContext,
        arena: &mut BoundedArena,
        res: &mut ResourceManager<S>,
    ) -> Self {
        let active_scene = otr_play_spawn_scene(initial_scene, arena, res);
        let baseline_scene_arena_used = if initial_scene == SceneId::SouthClockTown {
            Some(active_scene.arena_used_after_spawn)
        } else {
            None
        };
        let player = Player::new(active_scene.spawn_pos, PlayerForm::DekuScrub);
        let clock = ThreeDayClock::new(save.day.max(1), save.time);
        Self {
            scene: active_scene,
            player,
            camera: Camera::new(),
            clock,
            save,
            logic_ticks: 0,
            camera_epoch: 1,
            owl_save_count: 0,
            scene_transition_count: 0,
            max_letterbox_bar_px: 0,
            scene_leak_bytes_max: 0,
            baseline_scene_arena_used,
        }
    }

    pub fn transition_to_scene<S: PakSource>(
        &mut self,
        next_scene: SceneId,
        arena: &mut BoundedArena,
        res: &mut ResourceManager<S>,
    ) {
        let new_scene = otr_play_spawn_scene(next_scene, arena, res);
        if next_scene == SceneId::SouthClockTown {
            if let Some(prev) = self.baseline_scene_arena_used {
                let leak = (new_scene.arena_used_after_spawn as isize) - (prev as isize);
                if leak > self.scene_leak_bytes_max {
                    self.scene_leak_bytes_max = leak;
                }
            } else {
                self.baseline_scene_arena_used = Some(new_scene.arena_used_after_spawn);
            }
        }
        self.scene = new_scene;
        self.player.pos = self.scene.spawn_pos;
        self.player.vel = [0.0; 3];
        self.player.exit_triggered = None;
        self.camera_epoch += 1;
        self.scene_transition_count += 1;
    }

    pub fn play_ocarina_song(&mut self, song: OcarinaSong) -> ClockEvent {
        match song {
            OcarinaSong::None => ClockEvent::None,
            OcarinaSong::InvertedSongOfTime => {
                self.clock.set_inverted_song(!self.clock.inverted_song_active);
                ClockEvent::None
            }
            OcarinaSong::SongOfDoubleTime => self.clock.song_of_double_time(),
            OcarinaSong::SongOfTime => {
                self.clock.song_of_time_reset(&mut self.save);
                self.camera_epoch += 1;
                ClockEvent::DawnTransition(1)
            }
        }
    }

    /// Run one 20 Hz gameplay logic tick inside `PlayState`.
    pub fn update_logic<S: PakSource, F: FlashIo>(
        &mut self,
        pad: &PadInput,
        song: OcarinaSong,
        res: &mut ResourceManager<S>,
        arena: &mut BoundedArena,
        flash: &mut SysFlashrom<F>,
        interp: &mut FrameInterpolator,
    ) -> PlayStepEvent {
        self.logic_ticks += 1;
        interp.begin_tick(
            GameStateId::Play as u32,
            self.scene.id as i32,
            0,
            self.camera_epoch,
        );

        let mut clock_event = self.clock.tick();
        if song != OcarinaSong::None {
            clock_event = self.play_ocarina_song(song);
        }
        self.clock.sync_to_save(&mut self.save);

        self.player.update_logic(pad, &self.scene.collision, interp);
        self.camera.update_logic(
            self.player.pos,
            self.player.yaw,
            self.player.z_target_active,
            interp,
        );
        if self.camera.letterbox_bar_px > self.max_letterbox_bar_px {
            self.max_letterbox_bar_px = self.camera.letterbox_bar_px;
        }

        let interact_pressed = pad.pressed(n64_btn::A_BUTTON);
        let outcome: ActorStepOutcome = update_actors(
            &mut self.scene.actors,
            self.player.pos,
            interact_pressed,
            &mut self.save,
            self.scene.id,
        );

        let mut owl_saved = false;
        if outcome.owl_save_requested {
            let _ = flash.write_save_slot(0, &self.save);
            let _ = flash.await_result();
            owl_saved = true;
            self.owl_save_count += 1;
        }

        // Scene transition via door actor OR BgCheck exit polygon
        let next_scene = outcome.door_scene_transition.or_else(|| match self.player.exit_triggered {
            Some(1) => Some(SceneId::SouthClockTown),
            Some(2) => Some(SceneId::InsideTower),
            _ => None,
        });

        if let Some(target_scene) = next_scene {
            self.transition_to_scene(target_scene, arena, res);
        }

        interp.end_tick();

        let play_night_bell_sfx = matches!(clock_event, ClockEvent::NightStarted(_));
        PlayStepEvent {
            clock_event,
            scene_transition: next_scene,
            owl_saved,
            play_night_bell_sfx,
        }
    }

    pub fn draw_frame<S: PakSource, B: GfxBackend>(
        &self,
        fraction: f32,
        pool: &mut GfxPool,
        renderer: &mut Renderer,
        res: &mut ResourceManager<S>,
        backend: &mut B,
        interp: &mut FrameInterpolator,
    ) {
        let mut target = mat4_identity();
        target[3][0] = self.player.pos[0];
        target[3][1] = self.player.pos[1];
        target[3][2] = self.player.pos[2];
        let p_lerp = interp.resolve(PLAYER_ENTITY_ID, 0, &target, fraction);
        let px = p_lerp[3][0].clamp(-200.0, 200.0) as i16;
        let pz = (p_lerp[3][2] * 0.25).clamp(-90.0, 90.0) as i16;

        // 1. Clock Town Plaza / Scene ground quad
        let sky_rgba = if self.scene.id == SceneId::InsideTower {
            [50, 40, 70, 255]
        } else if self.clock.time >= 0xC000 || self.clock.time < 0x4000 {
            [24, 34, 68, 255]
        } else {
            [85, 165, 110, 255]
        };
        render_f3dex2_quad_layer(pool, renderer, res, backend, -190, -80, 190, 80, sky_rgba);

        // 2. Active scene actors
        for actor in &self.scene.actors {
            if actor.collected {
                continue;
            }
            let ax = (actor.pos[0] * 0.45).clamp(-200.0, 200.0) as i16;
            let az = (actor.pos[2] * 0.22).clamp(-80.0, 80.0) as i16;
            render_f3dex2_quad_layer(
                pool,
                renderer,
                res,
                backend,
                ax - 8,
                az - 8,
                ax + 8,
                az + 8,
                actor.color_rgba(),
            );
        }

        // 3. Player marker (Green = Human, Brown/Leaf = Deku)
        let player_rgba = if self.player.form == PlayerForm::DekuScrub {
            [180, 130, 50, 255]
        } else {
            [50, 210, 80, 255]
        };
        render_f3dex2_quad_layer(pool, renderer, res, backend, px - 9, pz - 9, px + 9, pz + 9, player_rgba);

        // 4. OVERLAY_DISP HUD + Letterbox bars
        HudRenderer::draw(
            &self.save,
            &self.clock,
            self.camera.letterbox_bar_px,
            pool,
            renderer,
            res,
            backend,
        );
    }
}
