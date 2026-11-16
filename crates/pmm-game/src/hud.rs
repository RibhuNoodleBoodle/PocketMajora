use pmm_assets::{PakSource, ResourceManager};
use pmm_gfx::{GfxBackend, Renderer};

use crate::clock::ThreeDayClock;
use crate::save::SaveContext;
use crate::states::render_f3dex2_quad_layer;
use crate::thga::GfxPool;

/// `OVERLAY_DISP` HUD renderer (z_parameter.c):
/// Hearts, Magic, A/B/C Icons, Rupee Counter, Minimap, 3-Day Clock Dial, & Z-target letterbox bars.
pub struct HudRenderer;

impl HudRenderer {
    pub fn draw<S: PakSource, B: GfxBackend>(
        save: &SaveContext,
        clock: &ThreeDayClock,
        letterbox_bar_px: u16,
        pool: &mut GfxPool,
        renderer: &mut Renderer,
        res: &mut ResourceManager<S>,
        backend: &mut B,
    ) {
        // 1. Hearts row (top-left)
        let hearts = ((save.health / 0x10) as i16).clamp(1, 10);
        for i in 0..hearts {
            let x = -220 + i * 14;
            render_f3dex2_quad_layer(pool, renderer, res, backend, x, 106, x + 10, 116, [235, 45, 75, 255]);
        }

        // 2. Magic meter (green bar below hearts)
        let magic_w = ((save.magic.max(0) as i16) * 2).clamp(16, 96);
        render_f3dex2_quad_layer(pool, renderer, res, backend, -220, 94, -220 + magic_w, 101, [40, 210, 85, 255]);

        // 3. A / B / C action buttons (top-right)
        render_f3dex2_quad_layer(pool, renderer, res, backend, 165, 96, 182, 114, [60, 130, 240, 255]); // A
        render_f3dex2_quad_layer(pool, renderer, res, backend, 142, 92, 158, 108, [60, 200, 90, 255]);  // B
        render_f3dex2_quad_layer(pool, renderer, res, backend, 188, 96, 202, 110, [240, 205, 40, 255]); // C

        // 4. Rupee box (bottom-left)
        let rupee_col = if save.rupees > 0 { [60, 235, 90, 255] } else { [90, 110, 95, 255] };
        render_f3dex2_quad_layer(pool, renderer, res, backend, -220, -118, -172, -104, rupee_col);

        // 5. Minimap widget (bottom-right)
        render_f3dex2_quad_layer(pool, renderer, res, backend, 162, -120, 224, -76, [30, 55, 115, 200]);

        // 6. Three-Day Clock Dial (bottom-center: diamond + sun/moon marker angle from clock.time)
        let clock_col = match clock.day {
            1 => [225, 200, 75, 255],
            2 => [235, 145, 45, 255],
            _ => [225, 65, 55, 255],
        };
        render_f3dex2_quad_layer(pool, renderer, res, backend, -48, -122, 48, -94, clock_col);
        let hand_x = (((clock.time as i32 - 0x8000) * 38) / 32768) as i16;
        render_f3dex2_quad_layer(pool, renderer, res, backend, hand_x - 4, -114, hand_x + 4, -102, [250, 248, 225, 255]);

        // 7. Z-Target letterbox bars (exact `letterbox_bar_px` 0..=24 top and bottom)
        if letterbox_bar_px > 0 {
            let h = letterbox_bar_px as i16;
            render_f3dex2_quad_layer(pool, renderer, res, backend, -240, 136 - h, 240, 136, [0, 0, 0, 255]);
            render_f3dex2_quad_layer(pool, renderer, res, backend, -240, -136, 240, -136 + h, [0, 0, 0, 255]);
        }
    }
}
