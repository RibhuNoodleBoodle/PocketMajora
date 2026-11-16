use alloc::vec::Vec;
use pmm_assets::{BoundedArena, PakSource, ResourceManager, SegmentTable};
use pmm_gfx::math::mat4_identity;
use pmm_gfx::{GfxBackend, Renderer};
use pmm_schema::{GfxWord, N64Vtx, gbi};

use crate::graph::GameStateId;
use crate::interp::FrameInterpolator;
use crate::padmgr::{PadInput, n64_btn};
use crate::save::{FlashIo, SaveContext, SysFlashrom};
use crate::thga::GfxPool;

pub const FLASH_SAVE_WAIT_TICKS: u16 = 40; // 2.0s at 20 Hz N64 logic rate

/// Parsed message metadata loaded from `__OTR__seg/message_data_static` (real US MM ROM)
/// or fallback built-in US strings when running without a full message archive.
pub struct OTRMessageTable {
    pub loaded_from_pak: bool,
    pub raw_bytes_len: usize,
    pub entry_count: usize,
}

impl OTRMessageTable {
    pub fn load<S: PakSource>(res: &mut ResourceManager<S>, arena: &mut BoundedArena) -> Self {
        let mut raw_bytes_len = 0usize;
        let mut entry_count = 0usize;
        let mut loaded_from_pak = false;

        if let Ok((_hdr, bytes)) = res.load_by_name_into_arena("__OTR__seg/message_data_static", arena) {
            loaded_from_pak = true;
            raw_bytes_len = bytes.len();
            // US MM message_data_static delimits message strings with 0xBF (`[END]`).
            for &b in bytes.iter() {
                if b == 0xBF {
                    entry_count += 1;
                }
            }
            if entry_count == 0 && !bytes.is_empty() {
                entry_count = 1;
            }
        }

        Self {
            loaded_from_pak,
            raw_bytes_len,
            entry_count,
        }
    }

    pub fn day_title(&self, day: i32) -> &'static str {
        match day {
            1 => "Dawn of the First Day",
            2 => "Dawn of the Second Day",
            3 => "Dawn of the Final Day",
            _ => "Dawn of a New Day",
        }
    }

    pub fn day_subtitle(&self, day: i32) -> &'static str {
        match day {
            1 => "-72 Hours Remain-",
            2 => "-48 Hours Remain-",
            3 => "-24 Hours Remain-",
            _ => "-Time Flows Onward-",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScriptedInputStep {
    pub start_tick: u32,
    pub end_tick: u32,
    pub buttons_n64: u32,
    pub stick_x: i8,
    pub stick_y: i8,
}

/// Deterministic controller script injector feeding real N64 button edges into `PadMgr`.
#[derive(Default)]
pub struct ScriptedPadInjector {
    steps: Vec<ScriptedInputStep>,
    pub current_tick: u32,
}

impl ScriptedPadInjector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_press(&mut self, tick: u32, buttons_n64: u32) {
        self.steps.push(ScriptedInputStep {
            start_tick: tick,
            end_tick: tick,
            buttons_n64,
            stick_x: 0,
            stick_y: 0,
        });
    }

    pub fn push_hold(&mut self, start_tick: u32, end_tick: u32, buttons_n64: u32, sx: i8, sy: i8) {
        self.steps.push(ScriptedInputStep {
            start_tick,
            end_tick,
            buttons_n64,
            stick_x: sx,
            stick_y: sy,
        });
    }

    pub fn sample_tick(&mut self) -> (u32, i8, i8) {
        let t = self.current_tick;
        self.current_tick = self.current_tick.saturating_add(1);
        let mut buttons = 0u32;
        let mut sx = 0i8;
        let mut sy = 0i8;
        for step in &self.steps {
            if t >= step.start_tick && t <= step.end_tick {
                buttons |= step.buttons_n64;
                if step.stick_x != 0 {
                    sx = step.stick_x;
                }
                if step.stick_y != 0 {
                    sy = step.stick_y;
                }
            }
        }
        (buttons, sx, sy)
    }
}

/// Build an F3DEX2 quad DL in `pool.poly_opa` backed by segment `0x0E` vertices and submit to `pmm-gfx`.
pub fn render_f3dex2_quad_layer<S: PakSource, B: GfxBackend>(
    pool: &mut GfxPool,
    renderer: &mut Renderer,
    res: &mut ResourceManager<S>,
    backend: &mut B,
    x0: i16,
    y0: i16,
    x1: i16,
    y1: i16,
    rgba: [u8; 4],
) {
    pool.reset();
    let verts = [
        N64Vtx { ob: [x0, y0, 0], flag: 0, tc: [0, 0], cn: rgba },
        N64Vtx { ob: [x1, y0, 0], flag: 0, tc: [1024, 0], cn: rgba },
        N64Vtx { ob: [x1, y1, 0], flag: 0, tc: [1024, 1024], cn: rgba },
        N64Vtx { ob: [x0, y1, 0], flag: 0, tc: [0, 1024], cn: rgba },
    ];
    let mut vtx_bytes = [0u8; 64];
    for (i, v) in verts.iter().enumerate() {
        let base = i * 16;
        vtx_bytes[base..base + 2].copy_from_slice(&v.ob[0].to_ne_bytes());
        vtx_bytes[base + 2..base + 4].copy_from_slice(&v.ob[1].to_ne_bytes());
        vtx_bytes[base + 4..base + 6].copy_from_slice(&v.ob[2].to_ne_bytes());
        vtx_bytes[base + 6..base + 8].copy_from_slice(&v.flag.to_ne_bytes());
        vtx_bytes[base + 8..base + 10].copy_from_slice(&v.tc[0].to_ne_bytes());
        vtx_bytes[base + 10..base + 12].copy_from_slice(&v.tc[1].to_ne_bytes());
        vtx_bytes[base + 12..base + 16].copy_from_slice(&v.cn);
    }

    let _ = pool.poly_opa.push_cmd(GfxWord::new(
        ((gbi::G_SETOTHERMODE_L as u32) << 24) | (3 << 8) | 1,
        if rgba[3] < 255 { 0x0010_0000 } else { 0 },
    ));
    let _ = pool.poly_opa.push_cmd(GfxWord::new(
        ((gbi::G_VTX as u32) << 24) | (4 << 12) | 8,
        0x0E00_0000,
    ));
    let _ = pool.poly_opa.push_cmd(GfxWord::new(
        ((gbi::G_QUAD as u32) << 24) | (0 << 16) | (2 << 8) | 4,
        (0 << 16) | (4 << 8) | 6,
    ));
    let _ = pool.poly_opa.push_cmd(GfxWord::new((gbi::G_ENDDL as u32) << 24, 0));

    let mut segs = SegmentTable::new(0x8000_0000, 0x8080_0000);
    let _ = segs.set_segment(0x0E, &vtx_bytes);
    let mut tex_scratch = [0u8; 64];

    let _ = renderer.process_display_list(
        pool.poly_opa.head_cmds(),
        &segs,
        res,
        &mut tex_scratch,
        backend,
        0,
    );
}

// =============================================================================
// 1. TitleSetupState -> ConsoleLogoState
// =============================================================================

pub struct TitleSetupState {
    pub completed: bool,
}

impl TitleSetupState {
    pub fn new() -> Self {
        Self { completed: false }
    }

    pub fn update<F: FlashIo, S: PakSource>(
        &mut self,
        flash: &mut SysFlashrom<F>,
        res: &mut ResourceManager<S>,
        arena: &mut BoundedArena,
    ) -> (SaveContext, OTRMessageTable, GameStateId) {
        let _ = flash.init_flash();
        let messages = OTRMessageTable::load(res, arena);
        let save = SaveContext::new_quest("Link");
        self.completed = true;
        (save, messages, GameStateId::Title)
    }
}

impl Default for TitleSetupState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// 2. ConsoleLogoState (`ovl_title` / `z_title.c`)
// =============================================================================

pub struct ConsoleLogoState {
    pub cover_alpha: i16,
    pub add_alpha: i16,
    pub visible_duration: i16,
    pub timer: i16,
    pub exit: bool,
    pub logic_ticks: u32,
    pub fade_in_seen: bool,
    pub hold_seen: bool,
    pub fade_out_seen: bool,
}

impl Default for ConsoleLogoState {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleLogoState {
    pub fn new() -> Self {
        Self {
            cover_alpha: 255,
            add_alpha: -12,
            visible_duration: 200,
            timer: 400,
            exit: false,
            logic_ticks: 0,
            fade_in_seen: false,
            hold_seen: false,
            fade_out_seen: false,
        }
    }

    /// 20 Hz logic update matching `z_title.c`'s `ConsoleLogo_UpdateCounters`.
    /// Pressing `START` or `A` during hold skips straight to fade-out.
    pub fn update_logic(&mut self, pad: &PadInput, interp: &mut FrameInterpolator) -> Option<GameStateId> {
        self.logic_ticks += 1;
        interp.begin_tick(GameStateId::Title as u32, -1, 0, 1);

        if self.cover_alpha == 0 && self.visible_duration != 0 {
            self.hold_seen = true;
            if pad.pressed(n64_btn::START_BUTTON | n64_btn::A_BUTTON) {
                self.visible_duration = 0;
                self.add_alpha = 12;
            } else {
                self.timer -= 1;
                self.visible_duration -= 1;
                if self.timer == 0 {
                    self.timer = 400;
                }
            }
        } else {
            if self.add_alpha < 0 {
                self.fade_in_seen = true;
            } else {
                self.fade_out_seen = true;
            }
            self.cover_alpha += self.add_alpha;
            if self.cover_alpha <= 0 {
                self.cover_alpha = 0;
                self.add_alpha = 12;
            } else if self.cover_alpha >= 255 {
                self.cover_alpha = 255;
                self.exit = true;
            }
        }

        let mut mtx = mat4_identity();
        mtx[3][0] = self.cover_alpha as f32;
        interp.record(0x5449_544C, 0, &mtx);
        interp.end_tick();

        if self.exit {
            Some(GameStateId::FileChoose)
        } else {
            None
        }
    }

    pub fn draw_frame<S: PakSource, B: GfxBackend>(
        &self,
        pool: &mut GfxPool,
        renderer: &mut Renderer,
        res: &mut ResourceManager<S>,
        backend: &mut B,
        interp: &mut FrameInterpolator,
        fraction: f32,
    ) {
        let mut target = mat4_identity();
        target[3][0] = self.cover_alpha as f32;
        let lerped = interp.resolve(0x5449_544C, 0, &target, fraction);
        let alpha = lerped[3][0].clamp(0.0, 255.0) as u8;

        // N64 logo / Ship emblem backdrop quad + cover fade quad via F3DEX2
        render_f3dex2_quad_layer(pool, renderer, res, backend, -120, -60, 120, 60, [32, 64, 180, 255]);
        render_f3dex2_quad_layer(pool, renderer, res, backend, -160, -120, 160, 120, [0, 0, 0, alpha]);
    }
}

// =============================================================================
// 3. FileSelectState (`ovl_file_choose` / `z_file_select.h`)
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuMode {
    Init = 0,
    Config = 1,
    Select = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMode {
    FadeInStart = 0x00,
    FadeInEnd = 0x01,
    MainMenu = 0x02,
    SetupCopySource = 0x03,
    SelectCopySource = 0x04,
    SelectCopyDest = 0x07,
    CopyWaitForFlashSave = 0x0D,
    EraseSelect = 0x16,
    EraseWaitForFlashSave = 0x1D,
    NameEntry = 0x24,
    NameEntryWaitForFlashSave = 0x25,
    OptionsMenu = 0x29,
    OptionsWaitForFlashSave = 0x2A,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectMode {
    ConfirmFile = 3,
    FadeOut = 6,
    LoadGame = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSelectOptions {
    pub sound_mode: u8,     // 0=Stereo, 1=Headset, 2=Surround
    pub z_target_hold: bool,
}

impl Default for FileSelectOptions {
    fn default() -> Self {
        Self {
            sound_mode: 0,
            z_target_hold: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpOutcome {
    CreatedSlot(usize),
    CopiedSlot { src: usize, dst: usize },
    ErasedSlot(usize),
    LaunchedSlot(usize),
}

pub struct FileSelectState {
    pub menu_mode: MenuMode,
    pub config_mode: ConfigMode,
    pub select_mode: SelectMode,
    pub selected_slot: usize,
    pub copy_src_slot: Option<usize>,
    pub wait_timer: u16,
    pub fade_alpha: i16,
    pub slot_valid: [bool; 3],
    pub slots: [Option<SaveContext>; 3],
    pub pending_name: [u8; 8],
    pub options: FileSelectOptions,
    pub logic_ticks: u32,
    pub last_op: Option<FileOpOutcome>,
}

impl FileSelectState {
    pub fn new<F: FlashIo>(flash: &mut SysFlashrom<F>) -> Self {
        let _ = flash.init_flash();
        let s0 = flash.read_save_slot(0);
        let s1 = flash.read_save_slot(1);
        let s2 = flash.read_save_slot(2);
        let slot_valid = [s0.is_some(), s1.is_some(), s2.is_some()];
        Self {
            menu_mode: MenuMode::Init,
            config_mode: ConfigMode::FadeInStart,
            select_mode: SelectMode::ConfirmFile,
            selected_slot: 0,
            copy_src_slot: None,
            wait_timer: 0,
            fade_alpha: 255,
            slot_valid,
            slots: [s0, s1, s2],
            pending_name: *b"Link\x3e\x3e\x3e\x3e",
            options: FileSelectOptions::default(),
            logic_ticks: 0,
            last_op: None,
        }
    }

    pub fn reload_slots<F: FlashIo>(&mut self, flash: &SysFlashrom<F>) {
        for i in 0..3 {
            self.slots[i] = flash.read_save_slot(i);
            self.slot_valid[i] = self.slots[i].is_some();
        }
    }

    /// Begin creating a new quest on an empty slot (enters `CM_NAME_ENTRY` with `"Link"` default).
    pub fn begin_create_slot(&mut self, slot: usize, name: &str) -> bool {
        if slot >= 3 || self.slot_valid[slot] {
            return false;
        }
        self.selected_slot = slot;
        self.pending_name = [0x3E; 8];
        for (i, b) in name.as_bytes().iter().take(8).enumerate() {
            self.pending_name[i] = *b;
        }
        self.menu_mode = MenuMode::Config;
        self.config_mode = ConfigMode::NameEntry;
        true
    }

    /// Confirm name entry and start the 40-tick `SysFlashrom` async save wait.
    pub fn confirm_name_entry<F: FlashIo>(&mut self, flash: &mut SysFlashrom<F>) -> bool {
        if self.config_mode != ConfigMode::NameEntry {
            return false;
        }
        let mut len = 0usize;
        while len < 8 && self.pending_name[len] != 0x3E {
            len += 1;
        }
        let name_str = core::str::from_utf8(&self.pending_name[..len]).unwrap_or("Link");
        let ctx = SaveContext::new_quest(name_str);
        if flash.write_save_slot(self.selected_slot, &ctx) != 0 {
            return false;
        }
        self.wait_timer = FLASH_SAVE_WAIT_TICKS;
        self.config_mode = ConfigMode::NameEntryWaitForFlashSave;
        true
    }

    /// Start copying valid `src` slot into empty `dst` slot with 40-tick async save wait.
    pub fn start_copy_slot<F: FlashIo>(&mut self, flash: &mut SysFlashrom<F>, src: usize, dst: usize) -> bool {
        if src >= 3 || dst >= 3 || src == dst || !self.slot_valid[src] || self.slot_valid[dst] {
            return false;
        }
        let Some(ctx) = self.slots[src].clone() else {
            return false;
        };
        if flash.write_save_slot(dst, &ctx) != 0 {
            return false;
        }
        self.copy_src_slot = Some(src);
        self.selected_slot = dst;
        self.wait_timer = FLASH_SAVE_WAIT_TICKS;
        self.menu_mode = MenuMode::Config;
        self.config_mode = ConfigMode::CopyWaitForFlashSave;
        true
    }

    /// Start erasing `slot` with 40-tick async erase wait.
    pub fn start_erase_slot<F: FlashIo>(&mut self, flash: &mut SysFlashrom<F>, slot: usize) -> bool {
        if slot >= 3 || !self.slot_valid[slot] {
            return false;
        }
        if flash.erase_save_slot(slot) != 0 {
            return false;
        }
        self.selected_slot = slot;
        self.wait_timer = FLASH_SAVE_WAIT_TICKS;
        self.menu_mode = MenuMode::Config;
        self.config_mode = ConfigMode::EraseWaitForFlashSave;
        true
    }

    /// Launch an occupied save slot -> `DayTelopState`.
    pub fn launch_slot(&mut self, slot: usize) -> bool {
        if slot >= 3 || !self.slot_valid[slot] {
            return false;
        }
        self.selected_slot = slot;
        self.menu_mode = MenuMode::Select;
        self.select_mode = SelectMode::ConfirmFile;
        true
    }

    /// Negative test helper: corrupt primary slot -> verify backup recovery; then corrupt backup -> verify clean `None`.
    pub fn verify_corrupt_slot_recovery<F: FlashIo>(
        &mut self,
        flash: &mut SysFlashrom<F>,
        slot: usize,
    ) -> bool {
        let probe = SaveContext::new_quest("Probe");
        if flash.write_save_slot(slot, &probe) != 0 {
            return false;
        }
        let _ = flash.await_result();

        // Corrupt primary only -> backup slot must recover exact SaveContext
        flash.corrupt_primary_slot_for_test(slot);
        let recovered = flash.read_save_slot(slot) == Some(probe);

        // Corrupt backup too -> must return None safely without panic or garbage data
        flash.corrupt_backup_slot_for_test(slot);
        let invalid_handled = flash.read_save_slot(slot).is_none();

        let _ = flash.erase_save_slot(slot);
        let _ = flash.await_result();
        self.reload_slots(flash);
        recovered && invalid_handled
    }

    /// 20 Hz logic update advancing active File Select `ConfigMode` / `SelectMode` and flash waits.
    pub fn update_logic<F: FlashIo>(
        &mut self,
        pad: &PadInput,
        flash: &mut SysFlashrom<F>,
        interp: &mut FrameInterpolator,
    ) -> Option<(GameStateId, SaveContext)> {
        self.logic_ticks += 1;
        interp.begin_tick(GameStateId::FileChoose as u32, -1, 0, 1);

        let mut next = None;

        match self.menu_mode {
            MenuMode::Init => {
                self.fade_alpha = (self.fade_alpha - 25).max(0);
                if self.fade_alpha == 0 {
                    self.menu_mode = MenuMode::Config;
                    self.config_mode = ConfigMode::MainMenu;
                }
            }
            MenuMode::Config => match self.config_mode {
                ConfigMode::FadeInStart | ConfigMode::FadeInEnd => {
                    self.config_mode = ConfigMode::MainMenu;
                }
                ConfigMode::MainMenu => {
                    if pad.pressed(n64_btn::D_JPAD) {
                        self.selected_slot = (self.selected_slot + 1) % 3;
                    } else if pad.pressed(n64_btn::U_JPAD) {
                        self.selected_slot = (self.selected_slot + 2) % 3;
                    } else if pad.pressed(n64_btn::R_TRIG) {
                        self.config_mode = ConfigMode::OptionsMenu;
                    } else if pad.pressed(n64_btn::A_BUTTON | n64_btn::START_BUTTON) {
                        if self.slot_valid[self.selected_slot] {
                            self.menu_mode = MenuMode::Select;
                            self.select_mode = SelectMode::ConfirmFile;
                        } else {
                            let s = self.selected_slot;
                            self.begin_create_slot(s, "Link");
                        }
                    }
                }
                ConfigMode::NameEntry => {
                    if pad.pressed(n64_btn::A_BUTTON | n64_btn::START_BUTTON) {
                        self.confirm_name_entry(flash);
                    } else if pad.pressed(n64_btn::B_BUTTON) {
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
                ConfigMode::NameEntryWaitForFlashSave => {
                    if self.wait_timer > 0 {
                        self.wait_timer -= 1;
                    }
                    if self.wait_timer == 0 {
                        if flash.is_busy() == 1 {
                            let _ = flash.await_result();
                        }
                        self.reload_slots(flash);
                        self.last_op = Some(FileOpOutcome::CreatedSlot(self.selected_slot));
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
                ConfigMode::CopyWaitForFlashSave => {
                    if self.wait_timer > 0 {
                        self.wait_timer -= 1;
                    }
                    if self.wait_timer == 0 {
                        if flash.is_busy() == 1 {
                            let _ = flash.await_result();
                        }
                        self.reload_slots(flash);
                        if let Some(src) = self.copy_src_slot.take() {
                            self.last_op = Some(FileOpOutcome::CopiedSlot {
                                src,
                                dst: self.selected_slot,
                            });
                        }
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
                ConfigMode::EraseWaitForFlashSave => {
                    if self.wait_timer > 0 {
                        self.wait_timer -= 1;
                    }
                    if self.wait_timer == 0 {
                        if flash.is_busy() == 1 {
                            let _ = flash.await_result();
                        }
                        self.reload_slots(flash);
                        self.last_op = Some(FileOpOutcome::ErasedSlot(self.selected_slot));
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
                ConfigMode::OptionsMenu => {
                    if pad.pressed(n64_btn::A_BUTTON) {
                        self.options.z_target_hold = !self.options.z_target_hold;
                    } else if pad.pressed(n64_btn::B_BUTTON | n64_btn::START_BUTTON) {
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
                ConfigMode::OptionsWaitForFlashSave
                | ConfigMode::SetupCopySource
                | ConfigMode::SelectCopySource
                | ConfigMode::SelectCopyDest
                | ConfigMode::EraseSelect => {
                    if pad.pressed(n64_btn::B_BUTTON) {
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
            },
            MenuMode::Select => match self.select_mode {
                SelectMode::ConfirmFile => {
                    if pad.pressed(n64_btn::A_BUTTON | n64_btn::START_BUTTON) {
                        self.select_mode = SelectMode::FadeOut;
                    } else if pad.pressed(n64_btn::B_BUTTON) {
                        self.menu_mode = MenuMode::Config;
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
                SelectMode::FadeOut => {
                    self.fade_alpha = (self.fade_alpha + 40).min(255);
                    if self.fade_alpha >= 255 {
                        self.select_mode = SelectMode::LoadGame;
                    }
                }
                SelectMode::LoadGame => {
                    if let Some(ctx) = self.slots[self.selected_slot].clone() {
                        self.last_op = Some(FileOpOutcome::LaunchedSlot(self.selected_slot));
                        next = Some((GameStateId::DayTelop, ctx));
                    } else {
                        self.menu_mode = MenuMode::Config;
                        self.config_mode = ConfigMode::MainMenu;
                    }
                }
            },
        }

        let mut mtx = mat4_identity();
        mtx[3][1] = (self.selected_slot as f32) * 32.0;
        interp.record(0x4649_4C45, 0, &mtx);
        interp.end_tick();

        next
    }

    pub fn draw_frame<S: PakSource, B: GfxBackend>(
        &self,
        pool: &mut GfxPool,
        renderer: &mut Renderer,
        res: &mut ResourceManager<S>,
        backend: &mut B,
        interp: &mut FrameInterpolator,
        fraction: f32,
    ) {
        let mut target = mat4_identity();
        target[3][1] = (self.selected_slot as f32) * 32.0;
        let lerped = interp.resolve(0x4649_4C45, 0, &target, fraction);
        let cursor_y = lerped[3][1] as i16 - 48;

        // Blue/Gold File Select panel + interpolated cursor quad via F3DEX2
        render_f3dex2_quad_layer(pool, renderer, res, backend, -140, -90, 140, 90, [24, 48, 128, 255]);
        render_f3dex2_quad_layer(pool, renderer, res, backend, -110, cursor_y, 110, cursor_y + 22, [220, 185, 64, 220]);
    }
}

// =============================================================================
// 4. DayTelopState ("Dawn of the First Day -72 Hours Remain-")
// =============================================================================

pub struct DayTelopState {
    pub day: i32,
    pub timer: u16,
    pub duration_ticks: u16,
    pub title: &'static str,
    pub subtitle: &'static str,
}

impl DayTelopState {
    pub fn new(day: i32, msgs: &OTRMessageTable) -> Self {
        Self {
            day,
            timer: 0,
            duration_ticks: 40,
            title: msgs.day_title(day),
            subtitle: msgs.day_subtitle(day),
        }
    }

    pub fn update_logic(&mut self, interp: &mut FrameInterpolator) -> Option<GameStateId> {
        self.timer += 1;
        interp.begin_tick(GameStateId::DayTelop as u32, -1, 0, 1);
        let mut mtx = mat4_identity();
        mtx[3][0] = self.timer as f32;
        interp.record(0x4441_5954, 0, &mtx);
        interp.end_tick();

        if self.timer >= self.duration_ticks {
            Some(GameStateId::Play)
        } else {
            None
        }
    }

    pub fn draw_frame<S: PakSource, B: GfxBackend>(
        &self,
        pool: &mut GfxPool,
        renderer: &mut Renderer,
        res: &mut ResourceManager<S>,
        backend: &mut B,
        interp: &mut FrameInterpolator,
        fraction: f32,
    ) {
        let mut target = mat4_identity();
        target[3][0] = self.timer as f32;
        let lerped = interp.resolve(0x4441_5954, 0, &target, fraction);
        let fade_in = ((lerped[3][0] / 12.0).clamp(0.0, 1.0) * 255.0) as u8;
        render_f3dex2_quad_layer(pool, renderer, res, backend, -160, -120, 160, 120, [8, 8, 12, 255]);
        render_f3dex2_quad_layer(pool, renderer, res, backend, -120, -24, 120, 24, [245, 235, 200, fade_in]);
    }
}
