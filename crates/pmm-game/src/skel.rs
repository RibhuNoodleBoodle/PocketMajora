use alloc::vec::Vec;
use pmm_assets::{PakSource, ResourceManager, SegmentTable};
use pmm_gfx::math::{Mat4, mat4_identity, mat4_mul};
use pmm_gfx::{GfxBackend, Renderer};

use crate::assets_rom::RomVisualBank;

#[derive(Debug, Clone, Copy)]
pub struct SkelLimb {
    pub joint: [f32; 3],
    pub child: u8,
    pub sibling: u8,
    pub dl_addr: u32,
}

#[derive(Debug, Clone)]
pub struct ParsedSkeleton {
    pub limbs: Vec<SkelLimb>,
}

/// Parse N64 big-endian `FlexSkeletonHeader` (`object_link_child`, `object_link_nuts`, `object_owl`).
pub fn parse_flex_skeleton(seg_bytes: &[u8], skel_off: usize, is_lod_limb: bool) -> Option<ParsedSkeleton> {
    if skel_off + 9 > seg_bytes.len() {
        return None;
    }
    let table_ptr = u32::from_be_bytes([
        seg_bytes[skel_off],
        seg_bytes[skel_off + 1],
        seg_bytes[skel_off + 2],
        seg_bytes[skel_off + 3],
    ]);
    if (table_ptr >> 24) != 0x06 {
        return None;
    }
    let limb_count = (seg_bytes[skel_off + 4] as usize).min(48);
    let table_off = (table_ptr & 0x00FF_FFFF) as usize;
    if table_off + limb_count * 4 > seg_bytes.len() {
        return None;
    }

    let limb_struct_size = if is_lod_limb { 16 } else { 12 };
    let mut limbs = Vec::with_capacity(limb_count);
    for i in 0..limb_count {
        let lptr = u32::from_be_bytes([
            seg_bytes[table_off + i * 4],
            seg_bytes[table_off + i * 4 + 1],
            seg_bytes[table_off + i * 4 + 2],
            seg_bytes[table_off + i * 4 + 3],
        ]);
        let loff = (lptr & 0x00FF_FFFF) as usize;
        if loff + limb_struct_size > seg_bytes.len() {
            return None;
        }
        let jx = i16::from_be_bytes([seg_bytes[loff], seg_bytes[loff + 1]]) as f32;
        let jy = i16::from_be_bytes([seg_bytes[loff + 2], seg_bytes[loff + 3]]) as f32;
        let jz = i16::from_be_bytes([seg_bytes[loff + 4], seg_bytes[loff + 5]]) as f32;
        let child = seg_bytes[loff + 6];
        let sibling = seg_bytes[loff + 7];
        let dl_addr = u32::from_be_bytes([
            seg_bytes[loff + 8],
            seg_bytes[loff + 9],
            seg_bytes[loff + 10],
            seg_bytes[loff + 11],
        ]);
        limbs.push(SkelLimb {
            joint: [jx, jy, jz],
            child,
            sibling,
            dl_addr,
        });
    }
    Some(ParsedSkeleton { limbs })
}

fn mat4_translate(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = mat4_identity();
    m[3][0] = x;
    m[3][1] = y;
    m[3][2] = z;
    m
}

fn mat4_scale_uniform(s: f32) -> Mat4 {
    let mut m = mat4_identity();
    m[0][0] = s;
    m[1][1] = s;
    m[2][2] = s;
    m
}

fn mat4_rot_y(rad: f32) -> Mat4 {
    let c = libm::cosf(rad);
    let s = libm::sinf(rad);
    let mut m = mat4_identity();
    m[0][0] = c;
    m[0][2] = -s;
    m[2][0] = s;
    m[2][2] = c;
    m
}

fn mat4_rot_z(rad: f32) -> Mat4 {
    let c = libm::cosf(rad);
    let s = libm::sinf(rad);
    let mut m = mat4_identity();
    m[0][0] = c;
    m[0][1] = s;
    m[1][0] = -s;
    m[1][1] = c;
    m
}

fn draw_limb_recursive<B: GfxBackend, S: PakSource>(
    skel: &ParsedSkeleton,
    ns_tag: u64,
    idx: usize,
    parent_mtx: &Mat4,
    walk_phase: f32,
    segs: &SegmentTable,
    renderer: &mut Renderer,
    res: &mut ResourceManager<S>,
    tex_scratch: &mut [u8],
    backend: &mut B,
    depth: usize,
) {
    if idx >= skel.limbs.len() || depth > 24 {
        return;
    }
    let limb = skel.limbs[idx];
    let t = mat4_translate(limb.joint[0], limb.joint[1], limb.joint[2]);
    // Gentle procedural walk/idle swing on leg & arm limbs
    let swing = match idx {
        3 | 15 => libm::sinf(walk_phase) * 0.35,
        6 | 18 => -libm::sinf(walk_phase) * 0.35,
        _ => 0.0,
    };
    let local = if swing != 0.0 {
        mat4_mul(&mat4_rot_z(swing), &t)
    } else {
        t
    };
    let cur_mtx = mat4_mul(&local, parent_mtx);

    if limb.dl_addr != 0 && renderer.push_model_matrix(&cur_mtx, backend).is_ok() {
        let list_id = ns_tag ^ ((idx as u64) << 36) ^ (limb.dl_addr as u64);
        let _ = renderer.process_static_be_display_list(
            list_id,
            limb.dl_addr,
            segs,
            res,
            tex_scratch,
            backend,
        );
        let _ = renderer.pop_model_matrix(backend);
    }

    if limb.child != 255 {
        draw_limb_recursive(
            skel,
            ns_tag,
            limb.child as usize,
            &cur_mtx,
            walk_phase,
            segs,
            renderer,
            res,
            tex_scratch,
            backend,
            depth + 1,
        );
    }
    if limb.sibling != 255 {
        draw_limb_recursive(
            skel,
            ns_tag,
            limb.sibling as usize,
            parent_mtx,
            walk_phase,
            segs,
            renderer,
            res,
            tex_scratch,
            backend,
            depth + 1,
        );
    }
}

/// Render a real N64 3D hierarchical skeleton (`gLinkHumanSkel`, `gLinkDekuSkel`, `gOwlFlyingSkel`)
/// scaled at MM's authentic `Actor_SetScale(0.01)` with eyes/mouth segments `0x08`/`0x09` mapped.
pub fn draw_rom_skeleton_actor<B: GfxBackend, S: PakSource>(
    skel: &ParsedSkeleton,
    ns_tag: u64,
    obj_bytes: &[u8],
    bank: &RomVisualBank,
    world_pos: [f32; 3],
    yaw_rad: f32,
    walk_phase: f32,
    scale: f32,
    renderer: &mut Renderer,
    res: &mut ResourceManager<S>,
    backend: &mut B,
) {
    let mut segs = SegmentTable::new(0x8000_0000, 0x8080_0000);
    if let Some(gk) = bank.gameplay_keep.as_slice() {
        let _ = segs.set_segment(0x04, gk);
    }
    if let Some(fk) = bank.gameplay_field_keep.as_slice() {
        let _ = segs.set_segment(0x05, fk);
    }
    let _ = segs.set_segment(0x06, obj_bytes);
    // Link eyes (`0x0000`) & mouth (`0x4000`) live in segment 0x08 and 0x09 on N64 MM
    let _ = segs.set_segment(0x08, obj_bytes);
    if obj_bytes.len() > 0x4000 {
        let _ = segs.set_segment(0x09, &obj_bytes[0x4000..]);
    }

    let root = mat4_mul(
        &mat4_mul(&mat4_scale_uniform(scale), &mat4_rot_y(yaw_rad)),
        &mat4_translate(world_pos[0], world_pos[1], world_pos[2]),
    );

    renderer.set_tex_namespace(ns_tag);
    renderer.set_overlay_mode(false, backend);
    let mut tex_scratch = [0u8; 64];
    draw_limb_recursive(
        skel,
        ns_tag,
        0,
        &root,
        walk_phase,
        &segs,
        renderer,
        res,
        &mut tex_scratch,
        backend,
        0,
    );
    backend.set_model_matrix(&mat4_identity());
    renderer.set_tex_namespace(0);
}
