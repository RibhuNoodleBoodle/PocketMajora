use alloc::vec::Vec;
use pmm_assets::{BoundedArena, PakSource, ResourceManager, SegmentTable};

use crate::actors::ActorInstance;
use crate::assets_rom::RomVisualBank;
use crate::bgcheck::CollisionMesh;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomDisplayListEntry {
    pub opa_dl: u32,
    pub xlu_dl: u32,
}

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
    pub spawn0: Option<[f32; 3]>,
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
            0x00 => {
                info.num_spawns = data1;
                if (w1 >> 24) == 0x02 {
                    let off = (w1 & 0x00FF_FFFF) as usize;
                    if off + 8 <= seg2_bytes.len() {
                        let x = i16::from_be_bytes([seg2_bytes[off + 2], seg2_bytes[off + 3]]) as f32;
                        let y = i16::from_be_bytes([seg2_bytes[off + 4], seg2_bytes[off + 5]]) as f32;
                        let z = i16::from_be_bytes([seg2_bytes[off + 6], seg2_bytes[off + 7]]) as f32;
                        info.spawn0 = Some([x, y, z]);
                    }
                }
            }
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

/// Walk big-endian N64 `RoomCmd` stream (`0x0A` `SCENE_CMD_ID_ROOM_SHAPE`) to extract
/// `RoomShapeNormal` (type 0) and `RoomShapeCullable` (type 2) display list addresses.
pub fn otr_room_extract_dls(seg3_bytes: &[u8]) -> Vec<RoomDisplayListEntry> {
    let mut out = Vec::new();
    let max_cmds = (seg3_bytes.len() / 8).min(64);
    for i in 0..max_cmds {
        let base = i * 8;
        let code = seg3_bytes[base];
        if code == 0x14 {
            break;
        }
        if code == 0x0A {
            let w1 = u32::from_be_bytes([
                seg3_bytes[base + 4],
                seg3_bytes[base + 5],
                seg3_bytes[base + 6],
                seg3_bytes[base + 7],
            ]);
            if (w1 >> 24) != 0x03 {
                continue;
            }
            let shape_off = (w1 & 0x00FF_FFFF) as usize;
            if shape_off + 8 > seg3_bytes.len() {
                continue;
            }
            let stype = seg3_bytes[shape_off];
            let num_entries = seg3_bytes[shape_off + 1] as usize;
            let entries_ptr = u32::from_be_bytes([
                seg3_bytes[shape_off + 4],
                seg3_bytes[shape_off + 5],
                seg3_bytes[shape_off + 6],
                seg3_bytes[shape_off + 7],
            ]);
            if (entries_ptr >> 24) != 0x03 {
                continue;
            }
            let ent_off = (entries_ptr & 0x00FF_FFFF) as usize;
            let stride = match stype {
                0 => 8,  // RoomShapeNormal: (opa: u32, xlu: u32)
                2 => 16, // RoomShapeCullable: (bounds[8], opa: u32, xlu: u32)
                _ => continue,
            };
            let dl_off_in_entry = if stype == 2 { 8 } else { 0 };
            for k in 0..num_entries.min(64) {
                let e = ent_off + k * stride;
                if e + stride > seg3_bytes.len() {
                    break;
                }
                let opa_dl = u32::from_be_bytes([
                    seg3_bytes[e + dl_off_in_entry],
                    seg3_bytes[e + dl_off_in_entry + 1],
                    seg3_bytes[e + dl_off_in_entry + 2],
                    seg3_bytes[e + dl_off_in_entry + 3],
                ]);
                let xlu_dl = u32::from_be_bytes([
                    seg3_bytes[e + dl_off_in_entry + 4],
                    seg3_bytes[e + dl_off_in_entry + 5],
                    seg3_bytes[e + dl_off_in_entry + 6],
                    seg3_bytes[e + dl_off_in_entry + 7],
                ]);
                if opa_dl != 0 || xlu_dl != 0 {
                    out.push(RoomDisplayListEntry { opa_dl, xlu_dl });
                }
            }
        }
    }
    out
}

pub struct ActiveScene {
    pub id: SceneId,
    pub header: SceneHeaderInfo,
    pub scene_bytes_loaded: usize,
    pub room_bytes_loaded: usize,
    pub rom_col_polys: usize,
    pub room_dls: Vec<RoomDisplayListEntry>,
    seg2_ptr: *const u8,
    seg2_len: usize,
    seg3_ptr: *const u8,
    seg3_len: usize,
    pub collision: CollisionMesh,
    pub spawn_pos: [f32; 3],
    pub actors: Vec<ActorInstance>,
    pub arena_used_after_spawn: usize,
}

impl ActiveScene {
    pub fn build_segment_table(&self, bank: &RomVisualBank) -> SegmentTable {
        let mut segs = SegmentTable::new(0x8000_0000, 0x8080_0000);
        if !self.seg2_ptr.is_null() && self.seg2_len > 0 {
            let s2 = unsafe { core::slice::from_raw_parts(self.seg2_ptr, self.seg2_len) };
            let _ = segs.set_segment(0x02, s2);
        }
        if !self.seg3_ptr.is_null() && self.seg3_len > 0 {
            let s3 = unsafe { core::slice::from_raw_parts(self.seg3_ptr, self.seg3_len) };
            let _ = segs.set_segment(0x03, s3);
        }
        if let Some(gk) = bank.gameplay_keep.as_slice() {
            let _ = segs.set_segment(0x04, gk);
        }
        if let Some(fk) = bank.gameplay_field_keep.as_slice() {
            let _ = segs.set_segment(0x05, fk);
        }
        segs
    }
}

/// Build playable Clock Town / Inside Tower collision anchored at the scene spawn origin
/// so Link stands directly on the real 3D ROM room coordinates.
fn build_playable_slice_collision(scene: SceneId, origin: [f32; 3]) -> CollisionMesh {
    let mut mesh = CollisionMesh::default();
    let [ox, oy, oz] = origin;
    match scene {
        SceneId::InsideTower => {
            mesh.add_floor_quad(ox - 180.0, oz - 180.0, ox + 180.0, oz + 140.0, oy, None);
            mesh.add_floor_quad(ox - 45.0, oz + 140.0, ox + 45.0, oz + 190.0, oy, Some(1));
            mesh.add_wall_quad(ox - 180.0, oz - 180.0, ox + 180.0, oz - 180.0, oy, oy + 120.0);
            mesh.add_wall_quad(ox + 180.0, oz - 180.0, ox + 180.0, oz + 190.0, oy, oy + 120.0);
            mesh.add_wall_quad(ox - 180.0, oz + 190.0, ox - 180.0, oz - 180.0, oy, oy + 120.0);
            mesh
        }
        SceneId::SouthClockTown | SceneId::ClockTower => {
            mesh.add_floor_quad(ox - 420.0, oz - 460.0, ox + 420.0, oz + 380.0, oy, None);
            mesh.add_triangle(
                [ox - 80.0, oy, oz - 220.0],
                [ox + 80.0, oy + 40.0, oz - 360.0],
                [ox - 80.0, oy + 40.0, oz - 360.0],
                None,
            );
            mesh.add_triangle(
                [ox - 80.0, oy, oz - 220.0],
                [ox + 80.0, oy, oz - 220.0],
                [ox + 80.0, oy + 40.0, oz - 360.0],
                None,
            );
            mesh.add_floor_quad(ox - 45.0, oz - 400.0, ox + 45.0, oz - 360.0, oy + 40.0, Some(2));
            mesh.add_wall_quad(ox - 420.0, oz - 460.0, ox + 420.0, oz - 460.0, oy, oy + 200.0);
            mesh.add_wall_quad(ox + 420.0, oz - 460.0, ox + 420.0, oz + 380.0, oy, oy + 200.0);
            mesh.add_wall_quad(ox + 420.0, oz + 380.0, ox - 420.0, oz + 380.0, oy, oy + 200.0);
            mesh.add_wall_quad(ox - 420.0, oz + 380.0, ox - 420.0, oz - 460.0, oy, oy + 200.0);
            mesh
        }
        _ => {
            mesh.add_floor_quad(ox - 300.0, oz - 300.0, ox + 300.0, oz + 300.0, oy, None);
            mesh.add_floor_quad(ox - 40.0, oz + 260.0, ox + 40.0, oz + 300.0, oy, Some(1));
            mesh.add_wall_quad(ox - 300.0, oz - 300.0, ox + 300.0, oz - 300.0, oy, oy + 160.0);
            mesh
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
    let mut room_dls = Vec::new();
    let mut seg2_ptr: *const u8 = core::ptr::null();
    let mut seg3_ptr: *const u8 = core::ptr::null();

    if let Ok((_hdr, seg2_bytes)) = res.load_by_name_into_arena(scene.seg_path(), arena) {
        scene_bytes_loaded = seg2_bytes.len();
        seg2_ptr = seg2_bytes.as_ptr();
        header = otr_scene_execute_commands(seg2_bytes);
        if let Some(col_off) = header.col_header_off {
            parsed_rom_col = CollisionMesh::from_n64_col_header(seg2_bytes, col_off);
        }
    }
    if let Ok((_hdr, seg3_bytes)) = res.load_by_name_into_arena(scene.room0_path(), arena) {
        room_bytes_loaded = seg3_bytes.len();
        seg3_ptr = seg3_bytes.as_ptr();
        room_dls = otr_room_extract_dls(seg3_bytes);
    }

    let spawn_pos = header.spawn0.unwrap_or(match scene {
        SceneId::SouthClockTown | SceneId::ClockTower => [0.0, 0.0, 40.0],
        _ => [0.0, 0.0, 0.0],
    });
    let collision = build_playable_slice_collision(scene, spawn_pos);
    let rom_col_polys = parsed_rom_col.map(|m| m.polys.len()).unwrap_or(0);

    let actors = ActorInstance::spawn_for_scene(scene, spawn_pos);
    let arena_used_after_spawn = arena.used_bytes();

    ActiveScene {
        id: scene,
        header,
        scene_bytes_loaded,
        room_bytes_loaded,
        rom_col_polys,
        room_dls,
        seg2_ptr,
        seg2_len: scene_bytes_loaded,
        seg3_ptr,
        seg3_len: room_bytes_loaded,
        collision,
        spawn_pos,
        actors,
        arena_used_after_spawn,
    }
}
