#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec;
use core::f32::consts::PI;
use core::ptr;
use psp::Align16;
use psp::input::Controller;
use psp::io::File;
use psp::mem::alloc_user_bytes;
use psp::sys::{
    self, ClearBuffer, DepthFunc, DisplayPixelFormat, FrontFaceDirection, GuContextType,
    GuPrimitive, GuState, GuSyncBehavior, GuSyncMode, ScePspFVector3, ShadingModel,
    TexturePixelFormat, VertexType,
};
use psp::vram_alloc::get_vram_allocator;
use psp::{BUF_WIDTH, SCREEN_HEIGHT, SCREEN_WIDTH};

psp::module!("PocketMajoraProbe", 1, 0);

static mut LIST: Align16<[u32; 0x10000]> = Align16([0; 0x10000]);

#[repr(C, align(4))]
struct ColorVertex {
    color: u32,
    x: f32,
    y: f32,
    z: f32,
}

static TRIANGLE_VERTS: Align16<[ColorVertex; 3]> = Align16([
    ColorVertex {
        color: 0xFF00_00FF, // Red (ABGR)
        x: 0.0,
        y: 0.8,
        z: 0.0,
    },
    ColorVertex {
        color: 0xFF00_FF00, // Green
        x: 0.8,
        y: -0.8,
        z: 0.0,
    },
    ColorVertex {
        color: 0xFFFF_0000, // Blue
        x: -0.8,
        y: -0.8,
        z: 0.0,
    },
]);

fn psp_main() {
    psp::callback::setup_exit_callback().unwrap();

    // Query kernel free memory BEFORE Rust heap / partition allocations.
    let initial_total_free = unsafe { sys::sceKernelTotalFreeMemSize() };
    let initial_max_free = unsafe { sys::sceKernelMaxFreeMemSize() };

    // Test Rust heap allocation (`alloc` 8 MiB arena).
    let heap_probe = vec![0xAAu8; 65536];
    let vec_addr = heap_probe.as_ptr() as usize;

    // Test 16 MiB Partition 2 user allocation to verify Slim 64 MiB + MEMSIZE=1.
    let slim_probe = alloc_user_bytes(16 * 1024 * 1024, b"slim_probe\0");
    let (slim_ok, slim_str) = match &slim_probe {
        Ok(alloc_blk) => (true, format!("OK({:#010x})", alloc_blk.as_ptr() as usize)),
        Err(err) => (false, format!("ERR({:#010x})", *err as u32)),
    };
    drop(slim_probe);
    drop(heap_probe);

    // Initialize VRAM buffers & PSP GU.
    let allocator = get_vram_allocator().unwrap();
    let fbp0 = allocator
        .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm8888)
        .unwrap();
    let fbp1 = allocator
        .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm8888)
        .unwrap();
    let zbp = allocator
        .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm4444)
        .unwrap();

    let fbp0_offset = fbp0.as_mut_ptr_from_zero() as usize;

    unsafe {
        sys::sceGumLoadIdentity();
        sys::sceGuInit();
        sys::sceGuStart(GuContextType::Direct, &raw mut LIST.0 as *mut _);
        sys::sceGuDrawBuffer(
            DisplayPixelFormat::Psm8888,
            fbp0.as_mut_ptr_from_zero() as _,
            BUF_WIDTH as i32,
        );
        sys::sceGuDispBuffer(
            SCREEN_WIDTH as i32,
            SCREEN_HEIGHT as i32,
            fbp1.as_mut_ptr_from_zero() as _,
            BUF_WIDTH as i32,
        );
        sys::sceGuDepthBuffer(zbp.as_mut_ptr_from_zero() as _, BUF_WIDTH as i32);
        sys::sceGuOffset(2048 - (SCREEN_WIDTH / 2), 2048 - (SCREEN_HEIGHT / 2));
        sys::sceGuViewport(2048, 2048, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
        sys::sceGuDepthRange(65535, 0);
        sys::sceGuScissor(0, 0, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
        sys::sceGuEnable(GuState::ScissorTest);
        sys::sceGuDepthFunc(DepthFunc::GreaterOrEqual);
        sys::sceGuEnable(GuState::DepthTest);
        sys::sceGuFrontFace(FrontFaceDirection::Clockwise);
        sys::sceGuShadeModel(ShadingModel::Smooth);
        sys::sceGuEnable(GuState::ClipPlanes);
        sys::sceGuFinish();
        sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
        sys::sceDisplayWaitVblankStart();
        sys::sceGuDisplay(true);
    }

    // Initialize controller input.
    psp::input::enable_analog();
    let mut ctrl = Controller::new();

    let mut frames_rendered: u32 = 0;
    for frame in 0..60u32 {
        ctrl.update();

        unsafe {
            sys::sceGuStart(GuContextType::Direct, &raw mut LIST.0 as *mut _);
            sys::sceGuClearColor(0xFF33_2211);
            sys::sceGuClearDepth(0);
            sys::sceGuClear(ClearBuffer::COLOR_BUFFER_BIT | ClearBuffer::DEPTH_BUFFER_BIT);

            sys::sceGumMatrixMode(sys::MatrixMode::Projection);
            sys::sceGumLoadIdentity();
            sys::sceGumPerspective(60.0, 16.0 / 9.0, 0.5, 100.0);

            sys::sceGumMatrixMode(sys::MatrixMode::View);
            sys::sceGumLoadIdentity();

            sys::sceGumMatrixMode(sys::MatrixMode::Model);
            sys::sceGumLoadIdentity();
            let pos = ScePspFVector3 {
                x: 0.0,
                y: 0.0,
                z: -2.2,
            };
            let rot = ScePspFVector3 {
                x: 0.0,
                y: (frame as f32) * 3.0 * (PI / 180.0),
                z: 0.0,
            };
            sys::sceGumTranslate(&pos);
            sys::sceGumRotateXYZ(&rot);

            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VertexType::COLOR_8888 | VertexType::VERTEX_32BITF | VertexType::TRANSFORM_3D,
                3,
                ptr::null_mut(),
                &TRIANGLE_VERTS as *const Align16<_> as *const _,
            );

            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            sys::sceDisplayWaitVblankStart();
            sys::sceGuSwapBuffers();
        }
        frames_rendered += 1;
    }

    let status = if slim_ok {
        "PASS"
    } else {
        "SLIM_RAM_NOT_UNLOCKED"
    };
    let lx = ctrl.analog_x();
    let ly = ctrl.analog_y();
    let buttons = ctrl.raw().buttons.bits();

    let report = format!(
        "STATUS: {status}\n\
         INITIAL_TOTAL_FREE_BYTES: {initial_total_free}\n\
         INITIAL_MAX_FREE_BYTES: {initial_max_free}\n\
         RUST_VEC_64K_ADDR: {vec_addr:#010x}\n\
         SLIM_16MIB_ALLOC: {slim_str}\n\
         VRAM_FBP0_OFFSET: {fbp0_offset:#010x}\n\
         FRAMES_RENDERED: {frames_rendered}\n\
         LAST_CTRL_LX_LY_BUTTONS: {lx},{ly},{buttons:#010x}\n"
    );

    if let Ok(f) = File::create("boot_probe.log") {
        let mut off = 0usize;
        let bytes = report.as_bytes();
        while off < bytes.len() {
            match f.write(&bytes[off..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => off += n,
            }
        }
    }

    unsafe {
        sys::sceGuTerm();
        sys::sceKernelExitGame();
    }
}
