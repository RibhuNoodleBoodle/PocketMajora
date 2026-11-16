use core::slice;
use psp::mem::{Partition2Alloc, alloc_user_bytes};

pub const POOL_COMMON_BYTES: usize = 6 * 1024 * 1024;
pub const POOL_SCENE_BYTES: usize = 8 * 1024 * 1024;
pub const POOL_RENDERER_BYTES: usize = 6 * 1024 * 1024;
pub const POOL_AUDIO_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct PoolInfo {
    pub addr: usize,
    pub size: usize,
    pub align_64: bool,
}

/// Fixed Partition 2 user-memory pools allocated once at boot to prevent
/// runtime kernel-block fragmentation or the 1,500-block firmware hang.
pub struct PartitionPools {
    pub common_assets: Partition2Alloc<u8>,
    pub scene_object: Partition2Alloc<u8>,
    pub renderer_cache: Partition2Alloc<u8>,
    pub audio_pool: Partition2Alloc<u8>,
}

impl PartitionPools {
    pub fn allocate() -> Result<Self, i32> {
        let common_assets = alloc_user_bytes(POOL_COMMON_BYTES as u32, b"pmm_common\0")?;
        let scene_object = alloc_user_bytes(POOL_SCENE_BYTES as u32, b"pmm_scene\0")?;
        let renderer_cache = alloc_user_bytes(POOL_RENDERER_BYTES as u32, b"pmm_gfx\0")?;
        let audio_pool = alloc_user_bytes(POOL_AUDIO_BYTES as u32, b"pmm_audio\0")?;
        Ok(Self {
            common_assets,
            scene_object,
            renderer_cache,
            audio_pool,
        })
    }

    pub fn info(&self) -> [PoolInfo; 4] {
        [
            Self::inspect(&self.common_assets, POOL_COMMON_BYTES),
            Self::inspect(&self.scene_object, POOL_SCENE_BYTES),
            Self::inspect(&self.renderer_cache, POOL_RENDERER_BYTES),
            Self::inspect(&self.audio_pool, POOL_AUDIO_BYTES),
        ]
    }

    fn inspect(blk: &Partition2Alloc<u8>, size: usize) -> PoolInfo {
        let addr = blk.as_ptr() as usize;
        PoolInfo {
            addr,
            size,
            align_64: (addr & 63) == 0,
        }
    }

    /// Borrow the 8 MiB Scene/Object pool as a deterministic bump/mark/reset arena.
    pub fn scene_arena_mut(&mut self) -> SceneObjectArena<'_> {
        let ptr = self.scene_object.as_mut_ptr();
        // SAFETY: `self.scene_object` exclusively owns `POOL_SCENE_BYTES` bytes.
        let slice = unsafe { slice::from_raw_parts_mut(ptr, POOL_SCENE_BYTES) };
        SceneObjectArena::new(slice)
    }
}

/// Scoped bump arena with nested Scene & Room reset markers (0 fragmentation).
pub struct SceneObjectArena<'a> {
    buf: &'a mut [u8],
    offset: usize,
    scene_mark: usize,
    room_mark: usize,
}

impl<'a> SceneObjectArena<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            offset: 0,
            scene_mark: 0,
            room_mark: 0,
        }
    }

    pub fn mark_scene(&mut self) {
        self.scene_mark = self.offset;
        self.room_mark = self.offset;
    }

    pub fn reset_to_scene(&mut self) {
        self.offset = self.scene_mark;
        self.room_mark = self.scene_mark;
    }

    pub fn mark_room(&mut self) {
        self.room_mark = self.offset;
    }

    pub fn reset_to_room(&mut self) {
        self.offset = self.room_mark;
    }

    pub fn used_bytes(&self) -> usize {
        self.offset
    }

    pub fn alloc_aligned(&mut self, size: usize, align: usize) -> Option<&mut [u8]> {
        let align = align.max(16);
        debug_assert!(align.is_power_of_two());
        let base = self.buf.as_ptr() as usize;
        let cur_addr = base + self.offset;
        let aligned_addr = (cur_addr + align - 1) & !(align - 1);
        let pad = aligned_addr - cur_addr;
        let new_off = self.offset.checked_add(pad)?.checked_add(size)?;
        if new_off > self.buf.len() {
            return None;
        }
        let start = self.offset + pad;
        self.offset = new_off;
        Some(&mut self.buf[start..new_off])
    }
}

/// Negative test: request an impossibly large 48 MiB Partition 2 block after the
/// 28 MiB architectural pools are active; must return `Err(SCE_KERNEL_ERROR_NO_MEMORY)`
/// without panicking or corrupting active pools.
pub fn alloc_oversize_negative_test() -> Result<(), i32> {
    let blk = alloc_user_bytes(48 * 1024 * 1024, b"pmm_neg48m\0")?;
    drop(blk);
    Ok(())
}
