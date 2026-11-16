use crate::PspVertex;

pub type Mat4 = [[f32; 4]; 4];

pub const fn mat4_identity() -> Mat4 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn mat4_mul_scalar(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            out[r][c] =
                a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c] + a[r][3] * b[3][c];
        }
    }
    out
}

pub fn mat4_lerp_scalar(prev: &Mat4, curr: &Mat4, fraction: f32) -> Mat4 {
    if fraction >= 1.0 {
        return *curr;
    }
    if fraction <= 0.0 {
        return *prev;
    }
    let mut out = [[0.0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            out[r][c] = prev[r][c] + (curr[r][c] - prev[r][c]) * fraction;
        }
    }
    out
}

#[cfg(all(target_arch = "mips", feature = "psp"))]
#[repr(C, align(16))]
struct AlignedMat4([[f32; 4]; 4]);

#[cfg(all(target_arch = "mips", feature = "psp"))]
pub fn mat4_mul_vfpu(a: &Mat4, b: &Mat4) -> Mat4 {
    let aa = AlignedMat4(*a);
    let bb = AlignedMat4(*b);
    let mut out = AlignedMat4([[0.0f32; 4]; 4]);
    let a_ptr = aa.0.as_ptr();
    let b_ptr = bb.0.as_ptr();
    let out_ptr = out.0.as_mut_ptr();
    // SAFETY: `AlignedMat4` guarantees 16-byte alignment for `lv.q`/`sv.q`.
    // In PSP VFPU `vmmul.q M200, M000, M100` computes `M000^T * M100` or `M100 * M000`
    // matching `simd.rs` / `gum.rs`: let's verify exact order via `M100, M000`.
    unsafe {
        psp::vfpu_asm!(
            "lv.q C000, 0({a_ptr})",
            "lv.q C010, 16({a_ptr})",
            "lv.q C020, 32({a_ptr})",
            "lv.q C030, 48({a_ptr})",
            "lv.q C100, 0({b_ptr})",
            "lv.q C110, 16({b_ptr})",
            "lv.q C120, 32({b_ptr})",
            "lv.q C130, 48({b_ptr})",
            "vmmul.q M200, M100, M000",
            "sv.q C200, 0({out_ptr})",
            "sv.q C210, 16({out_ptr})",
            "sv.q C220, 32({out_ptr})",
            "sv.q C230, 48({out_ptr})",
            a_ptr = in(reg) a_ptr,
            b_ptr = in(reg) b_ptr,
            out_ptr = in(reg) out_ptr,
            options(nostack),
        );
    }
    out.0
}

#[cfg(not(all(target_arch = "mips", feature = "psp")))]
pub fn mat4_mul_vfpu(a: &Mat4, b: &Mat4) -> Mat4 {
    mat4_mul_scalar(a, b)
}

#[cfg(all(target_arch = "mips", feature = "psp"))]
pub fn mat4_lerp_vfpu(prev: &Mat4, curr: &Mat4, fraction: f32) -> Mat4 {
    if fraction >= 1.0 {
        return *curr;
    }
    if fraction <= 0.0 {
        return *prev;
    }
    let pp = AlignedMat4(*prev);
    let cc = AlignedMat4(*curr);
    let mut out = AlignedMat4([[0.0f32; 4]; 4]);
    let t_bits = fraction.to_bits();
    let p_ptr = pp.0.as_ptr();
    let c_ptr = cc.0.as_ptr();
    let o_ptr = out.0.as_mut_ptr();
    unsafe {
        psp::vfpu_asm!(
            "mtv {t}, S300",
            "lv.q C000, 0({p_ptr})",
            "lv.q C100, 0({c_ptr})",
            "vsub.q C200, C100, C000",
            "vscl.q C200, C200, S300",
            "vadd.q C200, C000, C200",
            "sv.q C200, 0({o_ptr})",

            "lv.q C010, 16({p_ptr})",
            "lv.q C110, 16({c_ptr})",
            "vsub.q C210, C110, C010",
            "vscl.q C210, C210, S300",
            "vadd.q C210, C010, C210",
            "sv.q C210, 16({o_ptr})",

            "lv.q C020, 32({p_ptr})",
            "lv.q C120, 32({c_ptr})",
            "vsub.q C220, C120, C020",
            "vscl.q C220, C220, S300",
            "vadd.q C220, C020, C220",
            "sv.q C220, 32({o_ptr})",

            "lv.q C030, 48({p_ptr})",
            "lv.q C130, 48({c_ptr})",
            "vsub.q C230, C130, C030",
            "vscl.q C230, C230, S300",
            "vadd.q C230, C030, C230",
            "sv.q C230, 48({o_ptr})",
            t = in(reg) t_bits,
            p_ptr = in(reg) p_ptr,
            c_ptr = in(reg) c_ptr,
            o_ptr = in(reg) o_ptr,
            options(nostack),
        );
    }
    out.0
}

#[cfg(not(all(target_arch = "mips", feature = "psp")))]
pub fn mat4_lerp_vfpu(prev: &Mat4, curr: &Mat4, fraction: f32) -> Mat4 {
    mat4_lerp_scalar(prev, curr, fraction)
}

pub fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    mat4_mul_vfpu(a, b)
}

pub fn mat4_lerp(prev: &Mat4, curr: &Mat4, fraction: f32) -> Mat4 {
    mat4_lerp_vfpu(prev, curr, fraction)
}

/// Column-major orthographic projection matching PSP GUM `ScePspFMatrix4` (`m[col][row]`).
pub fn mat4_ortho(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Mat4 {
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = 2.0 / (right - left);
    m[1][1] = 2.0 / (top - bottom);
    m[2][2] = -2.0 / (far - near);
    m[3][0] = -(right + left) / (right - left);
    m[3][1] = -(top + bottom) / (top - bottom);
    m[3][2] = -(far + near) / (far - near);
    m[3][3] = 1.0;
    m
}

/// OpenGL/PSP GUM column-major perspective projection (`m[col][row]`).
pub fn mat4_perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / libm::tanf(fovy_rad * 0.5);
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = f / aspect.max(0.1);
    m[1][1] = f;
    m[2][2] = (far + near) / (near - far);
    m[2][3] = -1.0;
    m[3][2] = (2.0 * far * near) / (near - far);
    m
}

/// Column-major right-handed `LookAt` view matrix matching PSP GUM `ScePspFMatrix4`.
pub fn mat4_look_at(eye: [f32; 3], at: [f32; 3], up: [f32; 3]) -> Mat4 {
    let fx = at[0] - eye[0];
    let fy = at[1] - eye[1];
    let fz = at[2] - eye[2];
    let fl = libm::sqrtf(fx * fx + fy * fy + fz * fz).max(1e-5);
    let f = [fx / fl, fy / fl, fz / fl];

    let sx = f[1] * up[2] - f[2] * up[1];
    let sy = f[2] * up[0] - f[0] * up[2];
    let sz = f[0] * up[1] - f[1] * up[0];
    let sl = libm::sqrtf(sx * sx + sy * sy + sz * sz).max(1e-5);
    let s = [sx / sl, sy / sl, sz / sl];

    let u = [
        s[1] * f[2] - s[2] * f[1],
        s[2] * f[0] - s[0] * f[2],
        s[0] * f[1] - s[1] * f[0],
    ];

    let mut m = mat4_identity();
    m[0][0] = s[0];
    m[1][0] = s[1];
    m[2][0] = s[2];
    m[0][1] = u[0];
    m[1][1] = u[1];
    m[2][1] = u[2];
    m[0][2] = -f[0];
    m[1][2] = -f[1];
    m[2][2] = -f[2];
    m[3][0] = -(s[0] * eye[0] + s[1] * eye[1] + s[2] * eye[2]);
    m[3][1] = -(u[0] * eye[0] + u[1] * eye[1] + u[2] * eye[2]);
    m[3][2] = f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2];
    m
}

/// Software near-plane triangle clipper (`z <= -near` is in front of camera, i.e., `depth = -v.z >= near`).
/// Returns `(emitted_vertices, triangle_count)` where `triangle_count` is `0`, `1` (3 verts), or `2` (6 verts).
pub fn clip_triangle_near_plane(tri: [PspVertex; 3], near_depth: f32) -> ([PspVertex; 6], usize) {
    let depths = [-tri[0].z, -tri[1].z, -tri[2].z];
    let mut inside_idx = [0usize; 3];
    let mut outside_idx = [0usize; 3];
    let mut inside_cnt = 0usize;
    let mut outside_cnt = 0usize;

    for i in 0..3 {
        if depths[i] >= near_depth {
            inside_idx[inside_cnt] = i;
            inside_cnt += 1;
        } else {
            outside_idx[outside_cnt] = i;
            outside_cnt += 1;
        }
    }

    let mut out = [PspVertex::default(); 6];
    match inside_cnt {
        0 => (out, 0),
        3 => {
            out[0] = tri[0];
            out[1] = tri[1];
            out[2] = tri[2];
            (out, 1)
        }
        1 => {
            let a = tri[inside_idx[0]];
            let b = tri[outside_idx[0]];
            let c = tri[outside_idx[1]];
            let da = depths[inside_idx[0]];
            let db = depths[outside_idx[0]];
            let dc = depths[outside_idx[1]];
            let tab = (near_depth - da) / (db - da);
            let tac = (near_depth - da) / (dc - da);
            out[0] = a;
            out[1] = lerp_vertex(&a, &b, tab);
            out[2] = lerp_vertex(&a, &c, tac);
            (out, 1)
        }
        2 => {
            let a = tri[inside_idx[0]];
            let b = tri[inside_idx[1]];
            let c = tri[outside_idx[0]];
            let da = depths[inside_idx[0]];
            let db = depths[inside_idx[1]];
            let dc = depths[outside_idx[0]];
            let tac = (near_depth - da) / (dc - da);
            let tbc = (near_depth - db) / (dc - db);
            let ac = lerp_vertex(&a, &c, tac);
            let bc = lerp_vertex(&b, &c, tbc);
            out[0] = a;
            out[1] = b;
            out[2] = ac;
            out[3] = b;
            out[4] = bc;
            out[5] = ac;
            (out, 2)
        }
        _ => (out, 0),
    }
}

fn lerp_vertex(a: &PspVertex, b: &PspVertex, t: f32) -> PspVertex {
    let lerp_chan = |ca: u8, cb: u8| -> u8 {
        let v = (ca as f32) + ((cb as f32) - (ca as f32)) * t;
        v.clamp(0.0, 255.0) as u8
    };
    let ca = a.color.to_le_bytes();
    let cb = b.color.to_le_bytes();
    let color = u32::from_le_bytes([
        lerp_chan(ca[0], cb[0]),
        lerp_chan(ca[1], cb[1]),
        lerp_chan(ca[2], cb[2]),
        lerp_chan(ca[3], cb[3]),
    ]);
    PspVertex {
        u: a.u + (b.u - a.u) * t,
        v: a.v + (b.v - a.v) * t,
        color,
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        z: a.z + (b.z - a.z) * t,
    }
}

/// Clock Town 12-bit tile coordinate wrapping (`0..4096` N64 RDP texel space).
#[inline]
pub const fn wrap_tile_12bit(coord: i32) -> i32 {
    coord & 0x0FFF
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectI32 {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

/// Compute top letterbox bar, center scene scissor, and bottom letterbox bar
/// with identical endpoint rounding so Z-targeting never leaves 1-px seams.
pub fn compute_letterbox_scissor(
    screen_w: i32,
    screen_h: i32,
    n64_bar_px: i32,
) -> (RectI32, RectI32, RectI32) {
    let clamped_bar = if n64_bar_px < 0 {
        0
    } else if n64_bar_px > 32 {
        32
    } else {
        n64_bar_px
    };
    // Scale 240p N64 bar height to PSP 272p using exact integer rounding: (bar * screen_h + 120) / 240
    let scaled_bar = (clamped_bar * screen_h + 120) / 240;
    let top_bar = RectI32 {
        x0: 0,
        y0: 0,
        x1: screen_w,
        y1: scaled_bar,
    };
    let bottom_y0 = screen_h - scaled_bar;
    let bottom_bar = RectI32 {
        x0: 0,
        y0: bottom_y0,
        x1: screen_w,
        y1: screen_h,
    };
    let scene_scissor = RectI32 {
        x0: 0,
        y0: top_bar.y1,
        x1: screen_w,
        y1: bottom_bar.y0,
    };
    (scene_scissor, top_bar, bottom_bar)
}
