use alloc::vec;
use alloc::vec::Vec;

pub const FLASH_BLOCK_SIZE: usize = 128;
pub const FLASH_TOTAL_PAGES: usize = 1024;
pub const FLASH_TOTAL_BYTES: usize = FLASH_BLOCK_SIZE * FLASH_TOTAL_PAGES; // 131,072 bytes (128 KiB)
pub const SAVE_SLOT_SIZE: usize = 0x2000; // 8 KiB (64 flash pages) per slot
pub const SAVE_SLOT_MAGIC: [u8; 6] = *b"ZELDA3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveContext {
    pub newf: [u8; 6],
    pub three_day_reset_count: u16,
    pub player_name: [u8; 8],
    pub health_capacity: u16,
    pub health: u16,
    pub magic: i8,
    pub rupees: i16,
    pub sword_health: u16,
    pub time: u16,
    pub day: i32,
    pub total_days: i32,
    pub owl_save_location: u8,
    pub event_inf: [u16; 8],
    pub week_event_reg: [u8; 100],
    pub checksum: u16,
}

impl Default for SaveContext {
    fn default() -> Self {
        Self::new_quest("Link")
    }
}

impl SaveContext {
    pub fn new_quest(name: &str) -> Self {
        let mut player_name = [0x3Eu8; 8]; // space-padded
        for (i, b) in name.as_bytes().iter().take(8).enumerate() {
            player_name[i] = *b;
        }
        let mut ctx = Self {
            newf: SAVE_SLOT_MAGIC,
            three_day_reset_count: 0,
            player_name,
            health_capacity: 0x30, // 3 hearts
            health: 0x30,
            magic: 0,
            rupees: 0,
            sword_health: 100,
            time: 0x4000, // 6:00 AM Dawn of First Day
            day: 1,
            total_days: 0,
            owl_save_location: 0,
            event_inf: [0; 8],
            week_event_reg: [0; 100],
            checksum: 0,
        };
        ctx.checksum = ctx.compute_checksum();
        ctx
    }

    pub fn serialize_slot(&self) -> [u8; SAVE_SLOT_SIZE] {
        let mut buf = [0u8; SAVE_SLOT_SIZE];
        buf[0..6].copy_from_slice(&self.newf);
        buf[6..8].copy_from_slice(&self.three_day_reset_count.to_le_bytes());
        buf[8..16].copy_from_slice(&self.player_name);
        buf[16..18].copy_from_slice(&self.health_capacity.to_le_bytes());
        buf[18..20].copy_from_slice(&self.health.to_le_bytes());
        buf[20] = self.magic as u8;
        buf[21] = self.owl_save_location;
        buf[22..24].copy_from_slice(&self.rupees.to_le_bytes());
        buf[24..26].copy_from_slice(&self.sword_health.to_le_bytes());
        buf[26..28].copy_from_slice(&self.time.to_le_bytes());
        buf[28..32].copy_from_slice(&self.day.to_le_bytes());
        buf[32..36].copy_from_slice(&self.total_days.to_le_bytes());
        for (i, &w) in self.event_inf.iter().enumerate() {
            let off = 36 + i * 2;
            buf[off..off + 2].copy_from_slice(&w.to_le_bytes());
        }
        buf[52..152].copy_from_slice(&self.week_event_reg);
        let csum = Self::compute_buf_checksum(&buf[..SAVE_SLOT_SIZE - 2]);
        buf[SAVE_SLOT_SIZE - 2..SAVE_SLOT_SIZE].copy_from_slice(&csum.to_le_bytes());
        buf
    }

    pub fn deserialize_slot(buf: &[u8]) -> Option<Self> {
        if buf.len() < SAVE_SLOT_SIZE {
            return None;
        }
        if buf[0..6] != SAVE_SLOT_MAGIC {
            return None;
        }
        let stored_csum = u16::from_le_bytes([buf[SAVE_SLOT_SIZE - 2], buf[SAVE_SLOT_SIZE - 1]]);
        let calc_csum = Self::compute_buf_checksum(&buf[..SAVE_SLOT_SIZE - 2]);
        if stored_csum != calc_csum {
            return None;
        }
        let mut newf = [0u8; 6];
        newf.copy_from_slice(&buf[0..6]);
        let mut player_name = [0u8; 8];
        player_name.copy_from_slice(&buf[8..16]);
        let mut event_inf = [0u16; 8];
        for (i, slot) in event_inf.iter_mut().enumerate() {
            let off = 36 + i * 2;
            *slot = u16::from_le_bytes([buf[off], buf[off + 1]]);
        }
        let mut week_event_reg = [0u8; 100];
        week_event_reg.copy_from_slice(&buf[52..152]);
        Some(Self {
            newf,
            three_day_reset_count: u16::from_le_bytes([buf[6], buf[7]]),
            player_name,
            health_capacity: u16::from_le_bytes([buf[16], buf[17]]),
            health: u16::from_le_bytes([buf[18], buf[19]]),
            magic: buf[20] as i8,
            owl_save_location: buf[21],
            rupees: i16::from_le_bytes([buf[22], buf[23]]),
            sword_health: u16::from_le_bytes([buf[24], buf[25]]),
            time: u16::from_le_bytes([buf[26], buf[27]]),
            day: i32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]),
            total_days: i32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]),
            event_inf,
            week_event_reg,
            checksum: stored_csum,
        })
    }

    pub fn compute_checksum(&self) -> u16 {
        let buf = self.serialize_slot();
        u16::from_le_bytes([buf[SAVE_SLOT_SIZE - 2], buf[SAVE_SLOT_SIZE - 1]])
    }

    fn compute_buf_checksum(slice: &[u8]) -> u16 {
        let mut sum: u16 = 0;
        for chunk in slice.chunks_exact(2) {
            sum = sum.wrapping_add(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        sum
    }
}

pub trait FlashIo {
    fn load_image(&mut self, buf: &mut [u8]) -> Result<(), i32>;
    fn flush_image(&mut self, buf: &[u8]) -> Result<(), i32>;
}

/// 128 KiB (`0x20000` bytes) Flashrom emulation implementing MM's exact two-step
/// async completion contract (`write_async` -> `is_busy() == 1` -> `await_result()`).
pub struct SysFlashrom<F: FlashIo> {
    io: F,
    data: Vec<u8>,
    initialized: bool,
    async_result: i32,
    result_pending: bool,
}

impl<F: FlashIo> SysFlashrom<F> {
    pub fn new(io: F) -> Self {
        Self {
            io,
            data: vec![0xFFu8; FLASH_TOTAL_BYTES],
            initialized: false,
            async_result: -1,
            result_pending: false,
        }
    }

    pub fn init_flash(&mut self) -> i32 {
        if !self.initialized {
            self.data.fill(0xFF);
            let _ = self.io.load_image(&mut self.data);
            self.initialized = true;
        }
        0
    }

    pub fn is_init(&self) -> bool {
        self.initialized
    }

    pub fn read(&self, dst: &mut [u8], page_num: u32, page_count: u32) -> i32 {
        let offset = (page_num as usize).saturating_mul(FLASH_BLOCK_SIZE);
        let size = (page_count as usize).saturating_mul(FLASH_BLOCK_SIZE);
        if !self.initialized
            || dst.len() < size
            || offset > FLASH_TOTAL_BYTES
            || size > FLASH_TOTAL_BYTES - offset
        {
            return -1;
        }
        dst[..size].copy_from_slice(&self.data[offset..offset + size]);
        0
    }

    pub fn write_async(&mut self, src: &[u8], page_num: u32, page_count: u32) -> i32 {
        let offset = (page_num as usize).saturating_mul(FLASH_BLOCK_SIZE);
        let size = (page_count as usize).saturating_mul(FLASH_BLOCK_SIZE);
        self.async_result = -1;
        self.result_pending = false;
        if !self.initialized
            || src.len() < size
            || offset > FLASH_TOTAL_BYTES
            || size > FLASH_TOTAL_BYTES - offset
        {
            self.result_pending = self.initialized;
            return -1;
        }
        self.data[offset..offset + size].copy_from_slice(&src[..size]);
        self.async_result = match self.io.flush_image(&self.data) {
            Ok(()) => 0,
            Err(_) => -1,
        };
        self.result_pending = true;
        self.async_result
    }

    pub fn is_busy(&self) -> i32 {
        if !self.initialized {
            -1
        } else if self.result_pending {
            1
        } else {
            0
        }
    }

    pub fn await_result(&mut self) -> i32 {
        if !self.initialized || !self.result_pending {
            return -1;
        }
        self.result_pending = false;
        self.async_result
    }

    pub fn write_sync(&mut self, src: &[u8], page_num: u32, page_count: u32) -> i32 {
        let _ = self.write_async(src, page_num, page_count);
        self.await_result()
    }

    /// Slots 0..3 occupy pages `[0, 64)`, `[64, 128)`, `[128, 192)`, plus backup slots at `[192, 384)`.
    pub fn write_save_slot(&mut self, slot: usize, ctx: &SaveContext) -> i32 {
        if slot >= 3 {
            return -1;
        }
        let bytes = ctx.serialize_slot();
        let page = (slot as u32) * 64;
        let backup_page = ((slot + 3) as u32) * 64;
        let _ = self.write_sync(&bytes, backup_page, 64);
        self.write_async(&bytes, page, 64)
    }

    pub fn read_save_slot(&self, slot: usize) -> Option<SaveContext> {
        if slot >= 3 {
            return None;
        }
        let mut buf = [0u8; SAVE_SLOT_SIZE];
        let page = (slot as u32) * 64;
        if self.read(&mut buf, page, 64) == 0
            && let Some(ctx) = SaveContext::deserialize_slot(&buf)
        {
            return Some(ctx);
        }
        // Fallback to backup copy
        let backup_page = ((slot + 3) as u32) * 64;
        if self.read(&mut buf, backup_page, 64) == 0 {
            return SaveContext::deserialize_slot(&buf);
        }
        None
    }

    pub fn corrupt_primary_slot_for_test(&mut self, slot: usize) {
        if slot < 3 {
            let off = slot * SAVE_SLOT_SIZE;
            self.data[off + SAVE_SLOT_SIZE - 1] ^= 0xFF;
        }
    }
}

/// In-memory `FlashIo` for host unit tests.
#[derive(Default)]
pub struct MemFlashIo {
    pub backing: Vec<u8>,
}

impl FlashIo for MemFlashIo {
    fn load_image(&mut self, buf: &mut [u8]) -> Result<(), i32> {
        if self.backing.len() == buf.len() {
            buf.copy_from_slice(&self.backing);
        }
        Ok(())
    }

    fn flush_image(&mut self, buf: &[u8]) -> Result<(), i32> {
        self.backing = buf.to_vec();
        Ok(())
    }
}
