use crate::math::{
    Mat4, clip_triangle_near_plane, mat4_identity, mat4_lerp, mat4_lerp_scalar, mat4_mul,
    mat4_mul_scalar,
};
use crate::PspVertex;

#[inline]
pub fn transform_vec4_scalar(m: &Mat4, v: &[f32; 4]) -> [f32; 4] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2] + m[0][3] * v[3],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2] + m[1][3] * v[3],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2] + m[2][3] * v[3],
        m[3][0] * v[0] + m[3][1] * v[1] + m[3][2] * v[2] + m[3][3] * v[3],
    ]
}

#[cfg(all(target_arch = "mips", feature = "psp"))]
#[repr(C, align(16))]
struct AlignedVec4([f32; 4]);

#[cfg(all(target_arch = "mips", feature = "psp"))]
#[repr(C, align(16))]
struct AlignedMat4(Mat4);

#[cfg(all(target_arch = "mips", feature = "psp"))]
pub fn transform_vec4_vfpu(m: &Mat4, v: &[f32; 4]) -> [f32; 4] {
    let mm = AlignedMat4(*m);
    let vv = AlignedVec4(*v);
    let mut out = AlignedVec4([0.0; 4]);
    let m_ptr = mm.0.as_ptr();
    let v_ptr = vv.0.as_ptr();
    let o_ptr = out.0.as_mut_ptr();
    // `lv.q C0i0` loads row `i` into column register `C0i0`; `vtfm4.q C110, M000, C100`
    // computes `dot(C0i0, C100) = dot(m[i], v)` matching `transform_vec4_scalar`.
    unsafe {
        psp::vfpu_asm!(
            "lv.q C000, 0({m_ptr})",
            "lv.q C010, 16({m_ptr})",
            "lv.q C020, 32({m_ptr})",
            "lv.q C030, 48({m_ptr})",
            "lv.q C100, 0({v_ptr})",
            "vtfm4.q C110, M000, C100",
            "sv.q C110, 0({o_ptr})",
            m_ptr = in(reg) m_ptr,
            v_ptr = in(reg) v_ptr,
            o_ptr = in(reg) o_ptr,
            options(nostack),
        );
    }
    out.0
}

#[cfg(not(all(target_arch = "mips", feature = "psp")))]
pub fn transform_vec4_vfpu(m: &Mat4, v: &[f32; 4]) -> [f32; 4] {
    transform_vec4_scalar(m, v)
}

#[inline]
pub fn transform_vec4(m: &Mat4, v: &[f32; 4]) -> [f32; 4] {
    transform_vec4_vfpu(m, v)
}

/// Compare VFPU `vmmul.q`, `vtfm4.q`, matrix `lerp`, and near-plane triangle clipper
/// against scalar IEEE-754 reference implementations, returning `max_abs_err`.
pub fn verify_vfpu_equivalence() -> f32 {
    let mut a = mat4_identity();
    a[0] = [1.2, 0.3, -0.4, 2.5];
    a[1] = [-0.2, 1.1, 0.5, -1.0];
    a[2] = [0.6, -0.3, 0.9, 4.0];
    a[3] = [0.0, 0.0, 0.0, 1.0];

    let mut b = mat4_identity();
    b[0] = [0.9, -0.1, 0.2, -3.0];
    b[1] = [0.4, 0.85, -0.25, 1.5];
    b[2] = [-0.3, 0.2, 1.05, 0.5];
    b[3] = [0.1, -0.2, 0.3, 1.0];

    let mut max_err = 0.0f32;

    let mul_s = mat4_mul_scalar(&a, &b);
    let mul_v = mat4_mul(&a, &b);
    for r in 0..4 {
        for c in 0..4 {
            let d = (mul_s[r][c] - mul_v[r][c]).abs();
            if d > max_err {
                max_err = d;
            }
        }
    }

    let lerp_s = mat4_lerp_scalar(&a, &b, 0.5);
    let lerp_v = mat4_lerp(&a, &b, 0.5);
    for r in 0..4 {
        for c in 0..4 {
            let d = (lerp_s[r][c] - lerp_v[r][c]).abs();
            if d > max_err {
                max_err = d;
            }
        }
    }

    let vec = [2.5, -1.25, 3.75, 1.0];
    let tv_s = transform_vec4_scalar(&a, &vec);
    let tv_v = transform_vec4(&a, &vec);
    for i in 0..4 {
        let d = (tv_s[i] - tv_v[i]).abs();
        if d > max_err {
            max_err = d;
        }
    }

    // Near-plane clipper sanity
    let tri = [
        PspVertex { x: 0.0, y: 0.0, z: -2.0, u: 0.0, v: 0.0, color: 0xFFFF_FFFF },
        PspVertex { x: 4.0, y: 0.0, z: 2.0, u: 1.0, v: 0.0, color: 0xFFFF_FFFF },
        PspVertex { x: -4.0, y: 2.0, z: 2.0, u: 0.0, v: 1.0, color: 0xFFFF_FFFF },
    ];
    let (_clipped_buf, clipped_count) = clip_triangle_near_plane(tri, 0.0);
    if clipped_count == 0 {
        max_err = max_err.max(1.0);
    }

    max_err
}
