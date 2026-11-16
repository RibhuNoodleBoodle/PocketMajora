use pmm_assets::{PakSource, ResourceManager};
use pmm_gfx::{GfxBackend, Renderer};

use crate::assets_rom::{RomVisualBank, draw_rom_sprite};
use crate::clock::ThreeDayClock;
use crate::save::SaveContext;
use crate::states::render_f3dex2_quad_layer;
use crate::thga::GfxPool;

/// `OVERLAY_DISP` HUD renderer (z_parameter.c):
/// Real `parameter_static` Hearts, Magic, A/B/C Icons, Rupee Counter, Minimap, 3-Day Clock Dial, & Letterbox bars.
pub struct HudRenderer;

impl HudRenderer {
    pub fn draw_with_bank<S: PakSource, B: GfxBackend>(
        bank: &RomVisualBank,
        save: &SaveContext,
        clock: &ThreeDayClock,
        letterbox_bar_px: u16,
        pool: &mut GfxPool,
        renderer: &mut Renderer,
        res: &mut ResourceManager<S>,
        backend: &mut B,
    ) {
        // 1. Hearts row (top-left) using real `gHeartFullTex` / `gHeartEmptyTex`
        let max_hearts = ((save.health_capacity / 0x10) as i16).clamp(3, 10);
        let filled_hearts = ((save.health / 0x10) as i16).clamp(0, max_hearts);
        for i in 0..max_hearts {
            let x = (-220 + i * 14) as f32;
            let is_full = i < filled_hearts;
            let sprite = if is_full { &bank.hud_heart_full } else { &bank.hud_heart_empty };
            if let Some(sp) = sprite {
                let tint = if is_full { [255, 70, 95, 255] } else { [180, 180, 180, 200] };
                draw_rom_sprite(sp, x, 106.0, x + 12.0, 118.0, tint, renderer, backend);
            } else if is_full {
                render_f3dex2_quad_layer(pool, renderer, res, backend, x as i16, 106, x as i16 + 10, 116, [235, 45, 75, 255]);
            }
        }

        // 2. Magic meter (green bar below hearts)
        let magic_w = ((save.magic.max(0) as i16) * 2).clamp(16, 96);
        render_f3dex2_quad_layer(pool, renderer, res, backend, -220, 94, -220 + magic_w, 101, [40, 210, 85, 255]);

        // 3. A / B / C action buttons (top-right) with `gButtonBackgroundTex`
        if let Some(btn) = &bank.hud_btn_bg {
            draw_rom_sprite(btn, 162.0, 92.0, 184.0, 114.0, [70, 145, 255, 255], renderer, backend);  // A
            draw_rom_sprite(btn, 138.0, 88.0, 158.0, 108.0, [75, 220, 105, 255], renderer, backend);  // B
            draw_rom_sprite(btn, 188.0, 96.0, 204.0, 112.0, [245, 210, 50, 255], renderer, backend);  // C
        } else {
            render_f3dex2_quad_layer(pool, renderer, res, backend, 165, 96, 182, 114, [60, 130, 240, 255]);
            render_f3dex2_quad_layer(pool, renderer, res, backend, 142, 92, 158, 108, [60, 200, 90, 255]);
            render_f3dex2_quad_layer(pool, renderer, res, backend, 188, 96, 202, 110, [240, 205, 40, 255]);
        }

        // 4. Rupee icon (bottom-left)
        if let Some(rupee) = &bank.hud_rupee {
            draw_rom_sprite(rupee, -220.0, -120.0, -202.0, -102.0, [70, 240, 95, 255], renderer, backend);
        } else {
            let rupee_col = if save.rupees > 0 { [60, 235, 90, 255] } else { [90, 110, 95, 255] };
            render_f3dex2_quad_layer(pool, renderer, res, backend, -220, -118, -172, -104, rupee_col);
        }

        // 5. Minimap widget (bottom-right)
        render_f3dex2_quad_layer(pool, renderer, res, backend, 162, -120, 224, -76, [30, 55, 115, 185]);

        // 6. Three-Day Clock Dial (bottom-center: diamond + real clock icon marker)
        let clock_col = match clock.day {
            1 => [225, 200, 75, 245],
            2 => [235, 145, 45, 245],
            _ => [225, 65, 55, 245],
        };
        render_f3dex2_quad_layer(pool, renderer, res, backend, -48, -122, 48, -94, clock_col);
        let hand_x = (((clock.time as i32 - 0x8000) * 38) / 32768) as f32;
        if let Some(cicon) = &bank.hud_clock_icon {
            draw_rom_sprite(cicon, hand_x - 7.0, -115.0, hand_x + 7.0, -101.0, [255, 250, 225, 255], renderer, backend);
        } else {
            render_f3dex2_quad_layer(pool, renderer, res, backend, hand_x as i16 - 4, -114, hand_x as i16 + 4, -102, [250, 248, 225, 255]);
        }

        // 7. Z-Target letterbox bars (exact `letterbox_bar_px` 0..=24 top and bottom)
        if letterbox_bar_px > 0 {
            let h = letterbox_bar_px as i16;
            render_f3dex2_quad_layer(pool, renderer, res, backend, -240, 136 - h, 240, 136, [0, 0, 0, 255]);
            render_f3dex2_quad_layer(pool, renderer, res, backend, -240, -136, 240, -136 + h, [0, 0, 0, 255]);
        }
    }

    pub fn draw<S: PakSource, B: GfxBackend>(
        save: &SaveContext,
        clock: &ThreeDayClock,
        letterbox_bar_px: u16,
        pool: &mut GfxPool,
        renderer: &mut Renderer,
        res: &mut ResourceManager<S>,
        backend: &mut B,
    ) {
        let empty = RomVisualBank::default();
        Self::draw_with_bank(&empty, save, clock, letterbox_bar_px, pool, renderer, res, backend);
    }
}
