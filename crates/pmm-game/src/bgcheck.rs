use alloc::vec::Vec;

pub const BGCHECK_Y_MIN: f32 = -32000.0;
pub const BGCHECK_Y_MAX: f32 = 32000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColPoly {
    pub vtx: [u16; 3],
    pub normal: [f32; 3],
    pub dist: f32,
    pub exit_index: Option<u8>,
}

impl ColPoly {
    #[inline]
    pub fn is_floor(&self) -> bool {
        self.normal[1] >= 0.5
    }

    #[inline]
    pub fn is_ceiling(&self) -> bool {
        self.normal[1] <= -0.5
    }

    #[inline]
    pub fn is_wall(&self) -> bool {
        self.normal[1].abs() < 0.5
    }
}

#[derive(Debug, Clone, Default)]
pub struct CollisionMesh {
    pub vertices: Vec<[f32; 3]>,
    pub polys: Vec<ColPoly>,
    pub min_bounds: [f32; 3],
    pub max_bounds: [f32; 3],
}

#[inline]
fn read_be_i16(buf: &[u8], off: usize) -> i16 {
    i16::from_be_bytes([buf[off], buf[off + 1]])
}

#[inline]
fn read_be_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([buf[off], buf[off + 1]])
}

#[inline]
fn read_be_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

impl CollisionMesh {
    /// Parse a real big-endian N64 `CollisionHeader` inside segment `0x02` (`scene_bytes`).
    /// Caps vertex and polygon reads safely so untrusted offsets never panic.
    pub fn from_n64_col_header(scene_bytes: &[u8], col_off: usize) -> Option<Self> {
        if col_off.saturating_add(0x2C) > scene_bytes.len() {
            return None;
        }
        let min_bounds = [
            read_be_i16(scene_bytes, col_off) as f32,
            read_be_i16(scene_bytes, col_off + 2) as f32,
            read_be_i16(scene_bytes, col_off + 4) as f32,
        ];
        let max_bounds = [
            read_be_i16(scene_bytes, col_off + 6) as f32,
            read_be_i16(scene_bytes, col_off + 8) as f32,
            read_be_i16(scene_bytes, col_off + 10) as f32,
        ];
        let num_vtx = (read_be_u16(scene_bytes, col_off + 12) as usize).min(4096);
        let vtx_seg_addr = read_be_u32(scene_bytes, col_off + 16);
        let num_poly = (read_be_u16(scene_bytes, col_off + 20) as usize).min(8192);
        let poly_seg_addr = read_be_u32(scene_bytes, col_off + 24);
        let surf_seg_addr = read_be_u32(scene_bytes, col_off + 28);

        if ((vtx_seg_addr >> 24) & 0xFF) != 2 || ((poly_seg_addr >> 24) & 0xFF) != 2 {
            return None;
        }
        let vtx_off = (vtx_seg_addr & 0x00FF_FFFF) as usize;
        let poly_off = (poly_seg_addr & 0x00FF_FFFF) as usize;
        if vtx_off.saturating_add(num_vtx * 6) > scene_bytes.len()
            || poly_off.saturating_add(num_poly * 16) > scene_bytes.len()
        {
            return None;
        }

        let mut vertices = Vec::with_capacity(num_vtx);
        for i in 0..num_vtx {
            let p = vtx_off + i * 6;
            vertices.push([
                read_be_i16(scene_bytes, p) as f32,
                read_be_i16(scene_bytes, p + 2) as f32,
                read_be_i16(scene_bytes, p + 4) as f32,
            ]);
        }

        let surf_base = if ((surf_seg_addr >> 24) & 0xFF) == 2 {
            Some((surf_seg_addr & 0x00FF_FFFF) as usize)
        } else {
            None
        };

        let mut polys = Vec::with_capacity(num_poly);
        for i in 0..num_poly {
            let p = poly_off + i * 16;
            let surf_type_idx = read_be_u16(scene_bytes, p) as usize;
            let a = read_be_u16(scene_bytes, p + 2) & 0x1FFF;
            let b = read_be_u16(scene_bytes, p + 4) & 0x1FFF;
            let c = read_be_u16(scene_bytes, p + 6) & 0x1FFF;
            if (a as usize) >= vertices.len()
                || (b as usize) >= vertices.len()
                || (c as usize) >= vertices.len()
            {
                continue;
            }
            let nx = (read_be_i16(scene_bytes, p + 8) as f32) / 32767.0;
            let ny = (read_be_i16(scene_bytes, p + 10) as f32) / 32767.0;
            let nz = (read_be_i16(scene_bytes, p + 12) as f32) / 32767.0;
            let dist = read_be_i16(scene_bytes, p + 14) as f32;

            let exit_index = surf_base.and_then(|sb| {
                let soff = sb.saturating_add(surf_type_idx * 8);
                if soff + 4 <= scene_bytes.len() {
                    let word0 = read_be_u32(scene_bytes, soff);
                    let exit = ((word0 >> 8) & 0x1F) as u8;
                    if exit > 0 { Some(exit) } else { None }
                } else {
                    None
                }
            });

            polys.push(ColPoly {
                vtx: [a, b, c],
                normal: [nx, ny, nz],
                dist,
                exit_index,
            });
        }

        Some(Self {
            vertices,
            polys,
            min_bounds,
            max_bounds,
        })
    }

    /// Add an indexed triangle computing unit normal and plane distance `-(n . v0)` automatically.
    pub fn add_triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], exit_index: Option<u8>) {
        let base = self.vertices.len() as u16;
        for v in [a, b, c] {
            if self.vertices.is_empty() {
                self.min_bounds = v;
                self.max_bounds = v;
            } else {
                for k in 0..3 {
                    if v[k] < self.min_bounds[k] {
                        self.min_bounds[k] = v[k];
                    }
                    if v[k] > self.max_bounds[k] {
                        self.max_bounds[k] = v[k];
                    }
                }
            }
            self.vertices.push(v);
        }
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut nx = ab[1] * ac[2] - ab[2] * ac[1];
        let mut ny = ab[2] * ac[0] - ab[0] * ac[2];
        let mut nz = ab[0] * ac[1] - ab[1] * ac[0];
        let len = libm::sqrtf(nx * nx + ny * ny + nz * nz);
        if len > 1e-5 {
            nx /= len;
            ny /= len;
            nz /= len;
        } else {
            nx = 0.0;
            ny = 1.0;
            nz = 0.0;
        }
        let dist = -(nx * a[0] + ny * a[1] + nz * a[2]);
        self.polys.push(ColPoly {
            vtx: [base, base + 1, base + 2],
            normal: [nx, ny, nz],
            dist,
            exit_index,
        });
    }

    pub fn add_floor_quad(&mut self, x0: f32, z0: f32, x1: f32, z1: f32, y: f32, exit_index: Option<u8>) {
        self.add_triangle([x0, y, z0], [x0, y, z1], [x1, y, z1], exit_index);
        self.add_triangle([x0, y, z0], [x1, y, z1], [x1, y, z0], exit_index);
    }

    pub fn add_wall_quad(&mut self, x0: f32, z0: f32, x1: f32, z1: f32, y0: f32, y1: f32) {
        self.add_triangle([x0, y0, z0], [x1, y0, z1], [x1, y1, z1], None);
        self.add_triangle([x0, y0, z0], [x1, y1, z1], [x0, y1, z0], None);
    }
}

/// 2D point-in-triangle check on XZ plane (counter-clockwise or clockwise agnostic).
#[inline]
fn point_in_tri_xz(px: f32, pz: f32, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> bool {
    let s1 = (px - b[0]) * (a[2] - b[2]) - (a[0] - b[0]) * (pz - b[2]);
    let s2 = (px - c[0]) * (b[2] - c[2]) - (b[0] - c[0]) * (pz - c[2]);
    let s3 = (px - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (pz - a[2]);
    let has_neg = (s1 < -1e-3) || (s2 < -1e-3) || (s3 < -1e-3);
    let has_pos = (s1 > 1e-3) || (s2 > 1e-3) || (s3 > 1e-3);
    !(has_neg && has_pos)
}

/// N64 `BgCheck` collision solver.
pub struct BgCheck;

impl BgCheck {
    /// `BgCheck_EntityRaycastFloor`: find highest floor surface at `(x, z)` at or below `y + 18.0`.
    /// Returns `(BGCHECK_Y_MIN, None)` when outside floor geometry.
    pub fn entity_raycast_floor(mesh: &CollisionMesh, x: f32, y: f32, z: f32) -> (f32, Option<u8>) {
        let mut best_y = BGCHECK_Y_MIN;
        let mut best_exit = None;

        for poly in &mesh.polys {
            if !poly.is_floor() || poly.normal[1].abs() < 1e-4 {
                continue;
            }
            let va = mesh.vertices[poly.vtx[0] as usize];
            let vb = mesh.vertices[poly.vtx[1] as usize];
            let vc = mesh.vertices[poly.vtx[2] as usize];
            if !point_in_tri_xz(x, z, va, vb, vc) {
                continue;
            }
            let surf_y = -(poly.normal[0] * x + poly.normal[2] * z + poly.dist) / poly.normal[1];
            if surf_y <= y + 18.0 && surf_y > best_y {
                best_y = surf_y;
                best_exit = poly.exit_index;
            }
        }
        (best_y, best_exit)
    }

    /// `BgCheck_EntitySphVsWall`: resolve horizontal sphere overlap against wall polygons (`|ny| < 0.5`).
    /// Returns the number of wall contacts pushed out.
    pub fn entity_sph_vs_wall(mesh: &CollisionMesh, pos: &mut [f32; 3], radius: f32, height: f32) -> u32 {
        let mut hits = 0u32;
        let check_y = pos[1] + height * 0.5;

        for poly in &mesh.polys {
            if !poly.is_wall() {
                continue;
            }
            let va = mesh.vertices[poly.vtx[0] as usize];
            let vb = mesh.vertices[poly.vtx[1] as usize];
            let vc = mesh.vertices[poly.vtx[2] as usize];
            let min_y = va[1].min(vb[1]).min(vc[1]) - radius;
            let max_y = va[1].max(vb[1]).max(vc[1]) + radius;
            if check_y < min_y || check_y > max_y {
                continue;
            }

            // Bounding XZ envelope with wall radius margin
            let min_x = va[0].min(vb[0]).min(vc[0]) - radius;
            let max_x = va[0].max(vb[0]).max(vc[0]) + radius;
            let min_z = va[2].min(vb[2]).min(vc[2]) - radius;
            let max_z = va[2].max(vb[2]).max(vc[2]) + radius;
            if pos[0] < min_x || pos[0] > max_x || pos[2] < min_z || pos[2] > max_z {
                continue;
            }

            let signed_dist = poly.normal[0] * pos[0]
                + poly.normal[1] * check_y
                + poly.normal[2] * pos[2]
                + poly.dist;

            if signed_dist.abs() < radius {
                let push = if signed_dist >= 0.0 {
                    radius - signed_dist
                } else {
                    -(radius + signed_dist)
                };
                let horiz_len = libm::sqrtf(poly.normal[0] * poly.normal[0] + poly.normal[2] * poly.normal[2]);
                if horiz_len > 1e-4 {
                    pos[0] += (poly.normal[0] / horiz_len) * push;
                    pos[2] += (poly.normal[2] / horiz_len) * push;
                    hits += 1;
                }
            }
        }
        hits
    }

    pub fn check_ceiling(mesh: &CollisionMesh, x: f32, y: f32, z: f32, height: f32) -> Option<f32> {
        let mut lowest = BGCHECK_Y_MAX;
        let mut found = false;
        for poly in &mesh.polys {
            if !poly.is_ceiling() || poly.normal[1].abs() < 1e-4 {
                continue;
            }
            let va = mesh.vertices[poly.vtx[0] as usize];
            let vb = mesh.vertices[poly.vtx[1] as usize];
            let vc = mesh.vertices[poly.vtx[2] as usize];
            if !point_in_tri_xz(x, z, va, vb, vc) {
                continue;
            }
            let surf_y = -(poly.normal[0] * x + poly.normal[2] * z + poly.dist) / poly.normal[1];
            if surf_y >= y && surf_y <= y + height && surf_y < lowest {
                lowest = surf_y;
                found = true;
            }
        }
        if found { Some(lowest) } else { None }
    }
}
