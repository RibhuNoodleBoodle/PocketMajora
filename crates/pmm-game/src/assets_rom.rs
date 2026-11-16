use alloc::vec::Vec;
use pmm_assets::{BoundedArena, PakSource, ResourceManager};
use pmm_gfx::n64_tex::{decode_n64_texture_to_rgba8888, next_pow2_u16};
use pmm_gfx::{GfxBackend, PspVertex, Renderer};
use pmm_schema::gbi;

#[derive(Clone)]
pub struct RomTexSprite {
    pub key: u64,
    pub po2_w: u16,
    pub po2_h: u16,
    pub u_max: f32,
    pub v_max: f32,
    pub rgba: Vec<u8>,
}

impl RomTexSprite {
    pub fn decode_from(
        key: u64,
        seg: &[u8],
        offset: usize,
        fmt: u8,
        siz: u8,
        w: u16,
        h: u16,
    ) -> Option<Self> {
        if offset >= seg.len() {
            return None;
        }
        let dummy_tlut = [0xFFFF_FFFFu32; 256];
        let (pw, ph, rgba) =
            decode_n64_texture_to_rgba8888(&seg[offset..], fmt, siz, w, h, &dummy_tlut)?;
        let po2_w = next_pow2_u16(pw);
        let po2_h = next_pow2_u16(ph);
        Some(Self {
            key,
            po2_w,
            po2_h,
            u_max: (w as f32) / (po2_w as f32),
            v_max: (h as f32) / (po2_h as f32),
            rgba,
        })
    }
}

#[derive(Clone, Copy)]
pub struct SegSpan {
    pub ptr: *const u8,
    pub len: usize,
}

impl Default for SegSpan {
    fn default() -> Self {
        Self {
            ptr: core::ptr::null(),
            len: 0,
        }
    }
}

impl SegSpan {
    pub fn as_slice(&self) -> Option<&[u8]> {
        if self.ptr.is_null() || self.len == 0 {
            None
        } else {
            Some(unsafe { core::slice::from_raw_parts(self.ptr, self.len) })
        }
    }
}

fn pin_segment<S: PakSource>(
    res: &mut ResourceManager<S>,
    arena: &mut BoundedArena,
    name: &str,
) -> SegSpan {
    if let Ok((_hdr, bytes)) = res.load_by_name_into_arena(name, arena) {
        SegSpan {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    } else {
        SegSpan::default()
    }
}

/// Decoded real US Majora's Mask title, file-select, HUD textures, and shared object/keep spans
/// backed cleanly inside Pool 1 (`CommonAssetsArena`).
pub struct RomVisualBank {
    pub gameplay_keep: SegSpan,
    pub gameplay_field_keep: SegSpan,
    pub object_link_child: SegSpan,
    pub object_link_nuts: SegSpan,
    pub object_owl: SegSpan,
    pub nintendo_rogo_static: SegSpan,

    pub title_mask: Option<RomTexSprite>,
    pub title_zelda: Option<RomTexSprite>,
    pub title_legend_of: Option<RomTexSprite>,
    pub title_subtitle: Option<RomTexSprite>,
    pub title_subtitle_mask: Option<RomTexSprite>,
    pub title_copyright: Option<RomTexSprite>,
    pub rogo_text: Option<RomTexSprite>,

    pub fs_prompt: Option<RomTexSprite>,
    pub fs_file1: Option<RomTexSprite>,
    pub fs_file2: Option<RomTexSprite>,
    pub fs_file3: Option<RomTexSprite>,
    pub fs_copy: Option<RomTexSprite>,
    pub fs_erase: Option<RomTexSprite>,
    pub fs_options: Option<RomTexSprite>,

    pub hud_heart_full: Option<RomTexSprite>,
    pub hud_heart_empty: Option<RomTexSprite>,
    pub hud_rupee: Option<RomTexSprite>,
    pub hud_btn_bg: Option<RomTexSprite>,
    pub hud_clock_icon: Option<RomTexSprite>,
}

impl Default for RomVisualBank {
    fn default() -> Self {
        Self {
            gameplay_keep: SegSpan::default(),
            gameplay_field_keep: SegSpan::default(),
            object_link_child: SegSpan::default(),
            object_link_nuts: SegSpan::default(),
            object_owl: SegSpan::default(),
            nintendo_rogo_static: SegSpan::default(),
            title_mask: None,
            title_zelda: None,
            title_legend_of: None,
            title_subtitle: None,
            title_subtitle_mask: None,
            title_copyright: None,
            rogo_text: None,
            fs_prompt: None,
            fs_file1: None,
            fs_file2: None,
            fs_file3: None,
            fs_copy: None,
            fs_erase: None,
            fs_options: None,
            hud_heart_full: None,
            hud_heart_empty: None,
            hud_rupee: None,
            hud_btn_bg: None,
            hud_clock_icon: None,
        }
    }
}

impl RomVisualBank {
    pub fn load<S: PakSource>(res: &mut ResourceManager<S>, arena: &mut BoundedArena) -> Self {
        let mut bank = Self::default();
        bank.gameplay_keep = pin_segment(res, arena, "__OTR__seg/gameplay_keep");
        bank.gameplay_field_keep = pin_segment(res, arena, "__OTR__seg/gameplay_field_keep");
        bank.object_link_child = pin_segment(res, arena, "__OTR__seg/object_link_child");
        bank.object_link_nuts = pin_segment(res, arena, "__OTR__seg/object_link_nuts");
        bank.object_owl = pin_segment(res, arena, "__OTR__seg/object_owl");
        bank.nintendo_rogo_static = pin_segment(res, arena, "__OTR__seg/nintendo_rogo_static");

        if let Some(rogo) = bank.nintendo_rogo_static.as_slice() {
            bank.rogo_text =
                RomTexSprite::decode_from(0x524F_4701, rogo, 0x0, gbi::G_IM_FMT_I, gbi::G_IM_SIZ_8B, 192, 32);
        }

        let mag = pin_segment(res, arena, "__OTR__seg/object_mag");
        if let Some(m) = mag.as_slice() {
            bank.title_zelda =
                RomTexSprite::decode_from(0x4D41_4701, m, 0x0, gbi::G_IM_FMT_RGBA, gbi::G_IM_SIZ_32B, 144, 64);
            bank.title_subtitle =
                RomTexSprite::decode_from(0x4D41_4702, m, 0x9000, gbi::G_IM_FMT_I, gbi::G_IM_SIZ_8B, 104, 16);
            bank.title_subtitle_mask =
                RomTexSprite::decode_from(0x4D41_4706, m, 0x9680, gbi::G_IM_FMT_I, gbi::G_IM_SIZ_8B, 104, 16);
            bank.title_legend_of =
                RomTexSprite::decode_from(0x4D41_4703, m, 0x9D00, gbi::G_IM_FMT_I, gbi::G_IM_SIZ_8B, 72, 8);
            bank.title_copyright =
                RomTexSprite::decode_from(0x4D41_4704, m, 0x10F40, gbi::G_IM_FMT_I, gbi::G_IM_SIZ_8B, 128, 16);
            bank.title_mask =
                RomTexSprite::decode_from(0x4D41_4705, m, 0x11E48, gbi::G_IM_FMT_RGBA, gbi::G_IM_SIZ_32B, 128, 112);
        }

        let ts = pin_segment(res, arena, "__OTR__seg/title_static");
        if let Some(t) = ts.as_slice() {
            bank.fs_prompt =
                RomTexSprite::decode_from(0x5453_0001, t, 0x4980, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_8B, 128, 16);
            bank.fs_copy =
                RomTexSprite::decode_from(0x5453_0002, t, 0x1E7B0, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_16B, 64, 16);
            bank.fs_file1 =
                RomTexSprite::decode_from(0x5453_0003, t, 0x1EFB0, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_16B, 64, 16);
            bank.fs_file2 =
                RomTexSprite::decode_from(0x5453_0004, t, 0x1F7B0, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_16B, 64, 16);
            bank.fs_file3 =
                RomTexSprite::decode_from(0x5453_0005, t, 0x1FFB0, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_16B, 64, 16);
            bank.fs_erase =
                RomTexSprite::decode_from(0x5453_0006, t, 0x20FB0, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_16B, 64, 16);
            bank.fs_options =
                RomTexSprite::decode_from(0x5453_0007, t, 0x277B0, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_16B, 64, 16);
        }

        let ps = pin_segment(res, arena, "__OTR__seg/parameter_static");
        if let Some(p) = ps.as_slice() {
            bank.hud_heart_empty =
                RomTexSprite::decode_from(0x5053_0001, p, 0x0, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_8B, 16, 16);
            bank.hud_heart_full =
                RomTexSprite::decode_from(0x5053_0002, p, 0x400, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_8B, 16, 16);
            bank.hud_rupee =
                RomTexSprite::decode_from(0x5053_0003, p, 0xE60, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_8B, 16, 16);
            bank.hud_btn_bg =
                RomTexSprite::decode_from(0x5053_0004, p, 0xF60, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_8B, 32, 32);
            bank.hud_clock_icon =
                RomTexSprite::decode_from(0x5053_0005, p, 0x2360, gbi::G_IM_FMT_IA, gbi::G_IM_SIZ_8B, 16, 16);
        }

        bank
    }
}

/// Render a decoded `RomTexSprite` cleanly onto 2D ortho (`y+` upward, `-240..240, -136..136`)
/// with alpha blending and exact sub-rectangle UVs (`0..u_max, 0..v_max`).
pub fn draw_rom_sprite<B: GfxBackend>(
    sprite: &RomTexSprite,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    rgba_tint: [u8; 4],
    renderer: &mut Renderer,
    backend: &mut B,
) {
    renderer.set_overlay_mode(true, backend);
    let _ = backend.upload_rgba8888_texture(sprite.key, sprite.po2_w, sprite.po2_h, &sprite.rgba);
    backend.set_tex_sampler(true, true, false);
    backend.set_texture_enabled(true);
    backend.set_blend_xlu(true);

    let col = u32::from_le_bytes(rgba_tint);
    let u1 = sprite.u_max;
    let v1 = sprite.v_max;
    // Ortho has y0 (bottom) < y1 (top), so top edge maps v=0.0 and bottom edge maps v=v1
    let verts = [
        PspVertex { u: 0.0, v: v1, color: col, x: x0, y: y0, z: 0.0 },
        PspVertex { u: u1, v: v1, color: col, x: x1, y: y0, z: 0.0 },
        PspVertex { u: u1, v: 0.0, color: col, x: x1, y: y1, z: 0.0 },
        PspVertex { u: 0.0, v: v1, color: col, x: x0, y: y0, z: 0.0 },
        PspVertex { u: u1, v: 0.0, color: col, x: x1, y: y1, z: 0.0 },
        PspVertex { u: 0.0, v: 0.0, color: col, x: x0, y: y1, z: 0.0 },
    ];
    backend.draw_triangles(&verts);
    renderer.stats.triangles_submitted += 2;
}
