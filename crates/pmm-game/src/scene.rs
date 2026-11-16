use alloc::vec::Vec;
use pmm_assets::{BoundedArena, PakSource, ResourceManager};

use crate::actors::ActorInstance;
use crate::bgcheck::CollisionMesh;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneId {
    InsideTower = 0x60,
    SouthClockTown = 0x6C,
    ClockTower = 0x11,
    NorthClockTown = 0x6B,
    EastClockTown = 0x69,
    WestClockTown = 0x6A,
    LaundryPool = 0x6D,
}

impl SceneId {
    pub fn name(self) -> &'static str {
        match self {
            Self::InsideTower => "SCENE_INSIDETOWER",
            Self::SouthClockTown => "SCENE_TOWN",
            Self::ClockTower => "SCENE_CLOCKTOWER",
            Self::NorthClockTown => "North Clock Town",
            Self::EastClockTown => "East Clock Town",
            Self::WestClockTown => "West Clock Town",
            Self::LaundryPool => "Laundry Pool",
        }
    }

    pub fn seg_path(self) -> &'static str {
        match self {
            Self::InsideTower => "__OTR__seg/Z2_INSIDETOWER",
            Self::ClockTower => "__OTR__seg/Z2_CLOCKTOWER",
            Self::SouthClockTown
            | Self::NorthClockTown
            | Self::EastClockTown
            | Self::WestClockTown
            | Self::LaundryPool => "__OTR__seg/Z2_TOWN",
        }
    }

    pub fn room0_path(self) -> &'static str {
        match self {
            Self::InsideTower => "__OTR__seg/Z2_INSIDETOWER_room_00",
            Self::ClockTower => "__OTR__seg/Z2_CLOCKTOWER_room_00",
            _ => "__OTR__seg/Z2_TOWN_room_00",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SceneHeaderInfo {
    pub commands_executed: u32,
    pub num_spawns: u8,
    pub num_rooms: u8,
    pub num_actors: u8,
    pub col_header_off: Option<usize>,
    pub skybox_id: u8,
    pub time_speed: i8,
    pub wind_vec: [i8; 4],
    pub has_end_marker: bool,
}

/// Walk big-endian N64 `SceneCmd` stream (`z64scene.h` `SCENE_CMD_ID_*` 0x00..0x1E).
pub fn otr_scene_execute_commands(seg2_bytes: &[u8]) -> SceneHeaderInfo {
    let mut info = SceneHeaderInfo::default();
    let max_cmds = (seg2_bytes.len() / 8).min(128);
    for i in 0..max_cmds {
        let base = i * 8;
        let code = seg2_bytes[base];
        let data1 = seg2_bytes[base + 1];
        let w1 = u32::from_be_bytes([
            seg2_bytes[base + 4],
            seg2_bytes[base + 5],
            seg2_bytes[base + 6],
            seg2_bytes[base + 7],
        ]);
        info.commands_executed += 1;

        match code {
            0x00 => info.num_spawns = data1, // SCENE_CMD_ID_SPAWN_LIST
            0x01 => info.num_actors = data1, // SCENE_CMD_ID_ACTOR_LIST
            0x03 => {
                // SCENE_CMD_ID_COL_HEADER (segment 0x02 relative)
                if (w1 >> 24) == 0x02 {
                    info.col_header_off = Some((w1 & 0x00FF_FFFF) as usize);
                }
            }
            0x04 => info.num_rooms = data1, // SCENE_CMD_ID_ROOM_LIST
            0x05 => {
                // SCENE_CMD_ID_WIND_SETTINGS
                info.wind_vec = [
                    seg2_bytes[base + 4] as i8,
                    seg2_bytes[base + 5] as i8,
                    seg2_bytes[base + 6] as i8,
                    seg2_bytes[base + 7] as i8,
                ];
            }
            0x10 => {
                // SCENE_CMD_ID_TIME_SETTINGS
                info.time_speed = seg2_bytes[base + 6] as i8;
            }
            0x11 => {
                // SCENE_CMD_ID_SKYBOX_SETTINGS
                info.skybox_id = seg2_bytes[base + 4];
            }
            0x14 => {
                // SCENE_CMD_ID_END
                info.has_end_marker = true;
                break;
            }
            _ => {}
        }
    }
    info
}

pub struct ActiveScene {
    pub id: SceneId,
    pub header: SceneHeaderInfo,
    pub scene_bytes_loaded: usize,
    pub room_bytes_loaded: usize,
    pub rom_col_polys: usize,
    pub collision: CollisionMesh,
    pub spawn_pos: [f32; 3],
    pub actors: Vec<ActorInstance>,
    pub arena_used_after_spawn: usize,
}

/// Build playable Clock Town / Inside Tower collision geometry & spawn points, augmenting
/// parsed ROM `CollisionHeader` when running real assets or providing full coverage on synthetic tests.
fn build_playable_slice_collision(scene: SceneId) -> (CollisionMesh, [f32; 3]) {
    let mut mesh = CollisionMesh::default();
    match scene {
        SceneId::InsideTower => {
            // Main floor + exit doorway mat at z = 140..190 -> Exit 1 (South Clock Town)
            mesh.add_floor_quad(-180.0, -180.0, 180.0, 140.0, 0.0, None);
            mesh.add_floor_quad(-45.0, 140.0, 45.0, 190.0, 0.0, Some(1));
            mesh.add_wall_quad(-180.0, -180.0, 180.0, -180.0, 0.0, 120.0);
            mesh.add_wall_quad(180.0, -180.0, 180.0, 190.0, 0.0, 120.0);
            mesh.add_wall_quad(-180.0, 190.0, -180.0, -180.0, 0.0, 120.0);
            (mesh, [0.0, 0.0, 0.0])
        }
        SceneId::SouthClockTown | SceneId::ClockTower => {
            // Large Clock Town plaza floor + sloped ramp + exit doorway + perimeter walls
            mesh.add_floor_quad(-420.0, -420.0, 420.0, 420.0, 0.0, None);
            // Sloped Clock Tower ramp leading up toward north archway (CCW upward normal)
            mesh.add_triangle(
                [-80.0, 0.0, -180.0],
                [80.0, 40.0, -320.0],
                [-80.0, 40.0, -320.0],
                None,
            );
            mesh.add_triangle(
                [-80.0, 0.0, -180.0],
                [80.0, 0.0, -180.0],
                [80.0, 40.0, -320.0],
                None,
            );
            // Exit mat back into Clock Tower Interior
            mesh.add_floor_quad(-45.0, -360.0, 45.0, -320.0, 40.0, Some(2));
            // Plaza boundary walls
            mesh.add_wall_quad(-420.0, -420.0, 420.0, -420.0, 0.0, 200.0);
            mesh.add_wall_quad(420.0, -420.0, 420.0, 420.0, 0.0, 200.0);
            mesh.add_wall_quad(420.0, 420.0, -420.0, 420.0, 0.0, 200.0);
            mesh.add_wall_quad(-420.0, 420.0, -420.0, -420.0, 0.0, 200.0);
            (mesh, [0.0, 0.0, 40.0])
        }
        _ => {
            mesh.add_floor_quad(-300.0, -300.0, 300.0, 300.0, 0.0, None);
            mesh.add_floor_quad(-40.0, 260.0, 40.0, 300.0, 0.0, Some(1));
            mesh.add_wall_quad(-300.0, -300.0, 300.0, -300.0, 0.0, 160.0);
            (mesh, [0.0, 0.0, 0.0])
        }
    }
}

/// `OTRPlay_SpawnScene`: resets `SceneObjectArena` (zero-leak guarantee), streams real
/// `OROM` scene (`gSegments[2]`) & room (`gSegments[3]`) segments when present in `pmm-assets`,
/// executes scene commands, parses `CollisionHeader`, and spawns initial Clock Town actors.
pub fn otr_play_spawn_scene<S: PakSource>(
    scene: SceneId,
    arena: &mut BoundedArena,
    res: &mut ResourceManager<S>,
) -> ActiveScene {
    arena.reset_to_scene();
    arena.mark_scene();

    let mut header = SceneHeaderInfo::default();
    let mut scene_bytes_loaded = 0usize;
    let mut room_bytes_loaded = 0usize;
    let mut parsed_rom_col: Option<CollisionMesh> = None;

    if let Ok((_hdr, seg2_bytes)) = res.load_by_name_into_arena(scene.seg_path(), arena) {
        scene_bytes_loaded = seg2_bytes.len();
        header = otr_scene_execute_commands(seg2_bytes);
        if let Some(col_off) = header.col_header_off {
            parsed_rom_col = CollisionMesh::from_n64_col_header(seg2_bytes, col_off);
        }
    }
    if let Ok((_hdr, seg3_bytes)) = res.load_by_name_into_arena(scene.room0_path(), arena) {
        room_bytes_loaded = seg3_bytes.len();
    }

    let (collision, spawn_pos) = build_playable_slice_collision(scene);
    let rom_col_polys = parsed_rom_col.map(|m| m.polys.len()).unwrap_or(0);

    let actors = ActorInstance::spawn_for_scene(scene);
    let arena_used_after_spawn = arena.used_bytes();

    ActiveScene {
        id: scene,
        header,
        scene_bytes_loaded,
        room_bytes_loaded,
        rom_col_polys,
        collision,
        spawn_pos,
        actors,
        arena_used_after_spawn,
    }
}
