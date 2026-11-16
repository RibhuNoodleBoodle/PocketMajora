use alloc::vec::Vec;

use crate::save::SaveContext;
use crate::scene::SceneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorKind {
    ClockTowerDoor,  // En_Door
    OwlSaveStatue,   // Obj_Warpstone
    HappyMaskSalesman, // En_Ossan
    Tatl,            // En_Elf
    Banker,          // En_Ginko_Man
    Postbox,         // En_Pst
    Guard,           // En_Heishi
    CollectibleRupee, // En_Item00
    CollectibleHeart, // En_Item00
}

#[derive(Debug, Clone, Copy)]
pub struct ActorInstance {
    pub id: u64,
    pub kind: ActorKind,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub radius: f32,
    pub interacted: bool,
    pub collected: bool,
}

impl ActorInstance {
    pub fn color_rgba(&self) -> [u8; 4] {
        match self.kind {
            ActorKind::ClockTowerDoor => [120, 75, 40, 255],
            ActorKind::OwlSaveStatue => [200, 195, 175, 255],
            ActorKind::HappyMaskSalesman => [160, 80, 220, 255],
            ActorKind::Tatl => [255, 240, 110, 255],
            ActorKind::Banker => [235, 180, 45, 255],
            ActorKind::Postbox => [210, 40, 40, 255],
            ActorKind::Guard => [60, 110, 220, 255],
            ActorKind::CollectibleRupee => [40, 230, 80, 255],
            ActorKind::CollectibleHeart => [240, 60, 90, 255],
        }
    }

    pub fn spawn_for_scene(scene: SceneId, origin: [f32; 3]) -> Vec<Self> {
        let [ox, oy, oz] = origin;
        match scene {
            SceneId::InsideTower => alloc::vec![
                Self {
                    id: 0xAC01,
                    kind: ActorKind::HappyMaskSalesman,
                    pos: [ox, oy, oz - 70.0],
                    yaw: 0.0,
                    radius: 36.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xAC02,
                    kind: ActorKind::ClockTowerDoor,
                    pos: [ox, oy, oz + 155.0],
                    yaw: 3.14159,
                    radius: 42.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xAC03,
                    kind: ActorKind::Tatl,
                    pos: [ox + 18.0, oy + 32.0, oz + 10.0],
                    yaw: 0.0,
                    radius: 16.0,
                    interacted: false,
                    collected: false,
                },
            ],
            SceneId::SouthClockTown | SceneId::ClockTower => alloc::vec![
                Self {
                    id: 0xBC01,
                    kind: ActorKind::OwlSaveStatue,
                    pos: [ox + 95.0, oy, oz + 35.0],
                    yaw: 0.0,
                    radius: 40.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xBC02,
                    kind: ActorKind::ClockTowerDoor,
                    pos: [ox, oy + 40.0, oz - 375.0],
                    yaw: 0.0,
                    radius: 45.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xBC03,
                    kind: ActorKind::Banker,
                    pos: [ox - 120.0, oy, oz],
                    yaw: 1.57,
                    radius: 36.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xBC04,
                    kind: ActorKind::Postbox,
                    pos: [ox + 150.0, oy, oz - 60.0],
                    yaw: 0.0,
                    radius: 28.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xBC05,
                    kind: ActorKind::Guard,
                    pos: [ox, oy, oz + 220.0],
                    yaw: 3.14159,
                    radius: 36.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xBC06,
                    kind: ActorKind::CollectibleRupee,
                    pos: [ox + 40.0, oy, oz - 20.0],
                    yaw: 0.0,
                    radius: 28.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xBC07,
                    kind: ActorKind::CollectibleHeart,
                    pos: [ox - 40.0, oy, oz - 20.0],
                    yaw: 0.0,
                    radius: 28.0,
                    interacted: false,
                    collected: false,
                },
                Self {
                    id: 0xBC08,
                    kind: ActorKind::Tatl,
                    pos: [ox + 12.0, oy + 34.0, oz + 8.0],
                    yaw: 0.0,
                    radius: 16.0,
                    interacted: false,
                    collected: false,
                },
            ],
            _ => alloc::vec![Self {
                id: 0xCC01,
                kind: ActorKind::Guard,
                pos: [ox, oy, oz - 80.0],
                yaw: 0.0,
                radius: 36.0,
                interacted: false,
                collected: false,
            }],
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActorStepOutcome {
    pub owl_save_requested: bool,
    pub door_scene_transition: Option<SceneId>,
    pub collected_count: u32,
}

pub fn update_actors(
    actors: &mut [ActorInstance],
    player_pos: [f32; 3],
    interact_pressed: bool,
    save: &mut SaveContext,
    current_scene: SceneId,
) -> ActorStepOutcome {
    let mut out = ActorStepOutcome::default();

    for actor in actors.iter_mut() {
        if actor.collected {
            continue;
        }
        if actor.kind == ActorKind::Tatl {
            // Tatl orbits gently near Link
            actor.pos[0] = player_pos[0] + 16.0;
            actor.pos[1] = player_pos[1] + 34.0;
            actor.pos[2] = player_pos[2] + 8.0;
            continue;
        }

        let dx = actor.pos[0] - player_pos[0];
        let dz = actor.pos[2] - player_pos[2];
        let dist2 = dx * dx + dz * dz;
        if dist2 > actor.radius * actor.radius {
            continue;
        }

        match actor.kind {
            ActorKind::CollectibleRupee => {
                actor.collected = true;
                save.rupees = (save.rupees + 5).min(500);
                out.collected_count += 1;
            }
            ActorKind::CollectibleHeart => {
                actor.collected = true;
                save.health = (save.health + 0x10).min(save.health_capacity);
                out.collected_count += 1;
            }
            ActorKind::OwlSaveStatue if interact_pressed => {
                actor.interacted = true;
                save.owl_save_location = current_scene as u8;
                out.owl_save_requested = true;
            }
            ActorKind::ClockTowerDoor if interact_pressed => {
                actor.interacted = true;
                out.door_scene_transition = Some(if current_scene == SceneId::InsideTower {
                    SceneId::SouthClockTown
                } else {
                    SceneId::InsideTower
                });
            }
            ActorKind::HappyMaskSalesman | ActorKind::Banker | ActorKind::Postbox | ActorKind::Guard
                if interact_pressed =>
            {
                actor.interacted = true;
            }
            _ => {}
        }
    }

    out
}
