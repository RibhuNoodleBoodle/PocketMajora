#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;
use pmm_schema::{
    PAK_ENTRY_SIZE, PAK_HEADER_SIZE, PmmPakEntry, PmmPakHeader, ResourceId, SchemaError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceError {
    Io(i32),
    Schema(SchemaError),
    NotFound,
    OutOfMemory,
    TruncatedRead,
    FileClosed,
    AlignmentMismatch,
}

impl From<SchemaError> for ResourceError {
    fn from(e: SchemaError) -> Self {
        Self::Schema(e)
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(c) => write!(f, "I/O error {c:#x}"),
            Self::Schema(s) => write!(f, "schema error: {s}"),
            Self::NotFound => f.write_str("resource not found"),
            Self::OutOfMemory => f.write_str("arena out of memory"),
            Self::TruncatedRead => f.write_str("truncated pak read"),
            Self::FileClosed => f.write_str("pak file closed (suspended)"),
            Self::AlignmentMismatch => f.write_str("buffer alignment mismatch"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegError {
    NullAddress,
    UnmappedSegment(u8),
    OutOfBounds,
}

/// Abstract pak reader so PSP `sceIo*` (chunked 16 KiB + suspend/resume) and
/// host memory/file readers share identical parsing & verification logic.
pub trait PakSource {
    fn read_at(&mut self, offset: u32, dst: &mut [u8]) -> Result<(), ResourceError>;
    fn suspend_close(&mut self);
    fn resume_reopen(&mut self) -> Result<(), ResourceError>;
    fn is_open(&self) -> bool;
}

/// Bounded bump arena with nested mark/reset scopes (`CommonAssets` or `SceneObjectArena`).
pub struct BoundedArena {
    base: *mut u8,
    len: usize,
    offset: usize,
    scene_mark: usize,
    room_mark: usize,
}

// SAFETY: Managed single-owner arena buffer from Partition 2 or boxed slice.
unsafe impl Send for BoundedArena {}

impl BoundedArena {
    /// # Safety
    /// `base` must point to a valid mutable buffer of `len` bytes that outlives this `BoundedArena`.
    pub unsafe fn from_raw_parts(base: *mut u8, len: usize) -> Self {
        Self {
            base,
            len,
            offset: 0,
            scene_mark: 0,
            room_mark: 0,
        }
    }

    pub fn from_slice(buf: &mut [u8]) -> Self {
        Self {
            base: buf.as_mut_ptr(),
            len: buf.len(),
            offset: 0,
            scene_mark: 0,
            room_mark: 0,
        }
    }

    pub fn used_bytes(&self) -> usize {
        self.offset
    }

    pub fn capacity_bytes(&self) -> usize {
        self.len
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

    pub fn alloc_aligned(&mut self, size: usize, align: usize) -> Result<&mut [u8], ResourceError> {
        let align = align.max(16);
        if !align.is_power_of_two() {
            return Err(ResourceError::AlignmentMismatch);
        }
        let base_addr = self.base as usize;
        let cur_addr = base_addr + self.offset;
        let aligned_addr = (cur_addr + align - 1) & !(align - 1);
        let pad = aligned_addr - cur_addr;
        let new_off = self
            .offset
            .checked_add(pad)
            .and_then(|v| v.checked_add(size))
            .ok_or(ResourceError::OutOfMemory)?;
        if new_off > self.len {
            return Err(ResourceError::OutOfMemory);
        }
        let start = self.offset + pad;
        self.offset = new_off;
        // SAFETY: `start..new_off` lies strictly within `[0, self.len)`.
        Ok(unsafe { core::slice::from_raw_parts_mut(self.base.add(start), size) })
    }
}

#[derive(Clone, Copy)]
struct SegmentSlot {
    base: *const u8,
    len: usize,
    active: bool,
}

impl SegmentSlot {
    const fn empty() -> Self {
        Self {
            base: core::ptr::null(),
            len: 0,
            active: false,
        }
    }
}

/// N64 segment table (`gSegments[0..16]`) with safe disambiguation against
/// PSP native user-space pointers (`0x0880_0000..0x0C00_0000`).
pub struct SegmentTable {
    slots: [SegmentSlot; 16],
    native_ram_start: usize,
    native_ram_end: usize,
}

impl Default for SegmentTable {
    fn default() -> Self {
        Self::new(0x0880_0000, 0x0C00_0000)
    }
}

impl SegmentTable {
    pub const fn new(native_ram_start: usize, native_ram_end: usize) -> Self {
        Self {
            slots: [SegmentSlot::empty(); 16],
            native_ram_start,
            native_ram_end,
        }
    }

    pub fn set_segment(&mut self, seg: u8, data: &[u8]) {
        if (seg as usize) < 16 {
            self.slots[seg as usize] = SegmentSlot {
                base: data.as_ptr(),
                len: data.len(),
                active: true,
            };
        }
    }

    pub fn clear_segment(&mut self, seg: u8) {
        if (seg as usize) < 16 {
            self.slots[seg as usize] = SegmentSlot::empty();
        }
    }

    /// Translate an N64 segmented address (`0x0sOOOOOO`) or a native PSP user-space
    /// pointer (`native_ram_start..native_ram_end`) into `(ptr, remaining_bytes)`.
    pub fn resolve(&self, addr: u32) -> Result<(*const u8, usize), SegError> {
        if addr == 0 {
            return Err(SegError::NullAddress);
        }
        let seg = ((addr >> 24) & 0x0F) as usize;
        let off = (addr & 0x00FF_FFFF) as usize;
        let slot = self.slots[seg];
        if slot.active {
            if off < slot.len {
                // SAFETY: `off < slot.len` verified above.
                return Ok((unsafe { slot.base.add(off) }, slot.len - off));
            }
            // Only fall back to native PSP RAM if `addr` wasn't meant for a small active segment
            // Wait: if `seg` is active and `off >= slot.len`, treat as OutOfBounds unless inside native pool!
            let raw = addr as usize;
            if raw >= self.native_ram_start && raw < self.native_ram_end && seg >= 0x08 && seg <= 0x0B
            {
                return Ok((raw as *const u8, self.native_ram_end - raw));
            }
            return Err(SegError::OutOfBounds);
        }

        let raw = addr as usize;
        if raw >= self.native_ram_start && raw < self.native_ram_end {
            return Ok((raw as *const u8, self.native_ram_end - raw));
        }

        Err(SegError::UnmappedSegment(seg as u8))
    }
}

/// 2S2H-compatible resource manager reading `pmm_2s2h.pak` into bounded PSP arenas.
pub struct ResourceManager<S: PakSource> {
    source: S,
    header: PmmPakHeader,
    entries: Vec<PmmPakEntry>,
}

impl<S: PakSource> ResourceManager<S> {
    pub fn open(mut source: S) -> Result<Self, ResourceError> {
        let mut hdr_buf = [0u8; PAK_HEADER_SIZE];
        source.read_at(0, &mut hdr_buf)?;
        let header = PmmPakHeader::from_le_bytes(&hdr_buf)?;

        let count = header.entry_count as usize;
        let index_bytes_len = count
            .checked_mul(PAK_ENTRY_SIZE)
            .ok_or(ResourceError::TruncatedRead)?;
        let mut idx_buf = alloc::vec![0u8; index_bytes_len];
        source.read_at(header.index_offset, &mut idx_buf)?;

        let mut entries = Vec::with_capacity(count);
        for chunk in idx_buf.chunks_exact(PAK_ENTRY_SIZE) {
            entries.push(PmmPakEntry::from_le_bytes(chunk)?);
        }
        entries.sort_by_key(|e| e.id);

        Ok(Self {
            source,
            header,
            entries,
        })
    }

    pub fn header(&self) -> &PmmPakHeader {
        &self.header
    }

    pub fn entries(&self) -> &[PmmPakEntry] {
        &self.entries
    }

    pub fn otr_sig_check(path: &str) -> bool {
        path.starts_with("__OTR__")
    }

    pub fn lookup_hash(&self, id: ResourceId) -> Option<&PmmPakEntry> {
        self.entries
            .binary_search_by_key(&id.0, |e| e.id)
            .ok()
            .map(|idx| &self.entries[idx])
    }

    pub fn lookup_name(&self, otr_path: &str) -> Option<&PmmPakEntry> {
        self.lookup_hash(ResourceId::from_otr_path(otr_path))
    }

    /// Load resource payload into the specified bounded arena with 16-byte alignment.
    pub fn load_into_arena<'a>(
        &mut self,
        id: ResourceId,
        arena: &'a mut BoundedArena,
    ) -> Result<(PmmPakEntry, &'a mut [u8]), ResourceError> {
        let entry = *self.lookup_hash(id).ok_or(ResourceError::NotFound)?;
        let dst = arena.alloc_aligned(entry.size as usize, entry.align as usize)?;
        self.source.read_at(entry.offset, dst)?;
        Ok((entry, dst))
    }

    pub fn load_by_name_into_arena<'a>(
        &mut self,
        otr_path: &str,
        arena: &'a mut BoundedArena,
    ) -> Result<(PmmPakEntry, &'a mut [u8]), ResourceError> {
        self.load_into_arena(ResourceId::from_otr_path(otr_path), arena)
    }

    pub fn suspend_close(&mut self) {
        self.source.suspend_close();
    }

    pub fn resume_reopen(&mut self) -> Result<(), ResourceError> {
        self.source.resume_reopen()
    }

    pub fn is_open(&self) -> bool {
        self.source.is_open()
    }
}

/// In-memory `PakSource` for unit tests and host validation.
pub struct MemPakSource {
    data: Vec<u8>,
    open: bool,
}

impl MemPakSource {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, open: true }
    }
}

impl PakSource for MemPakSource {
    fn read_at(&mut self, offset: u32, dst: &mut [u8]) -> Result<(), ResourceError> {
        if !self.open {
            return Err(ResourceError::FileClosed);
        }
        let start = offset as usize;
        let end = start
            .checked_add(dst.len())
            .ok_or(ResourceError::TruncatedRead)?;
        if end > self.data.len() {
            return Err(ResourceError::TruncatedRead);
        }
        dst.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn suspend_close(&mut self) {
        self.open = false;
    }

    fn resume_reopen(&mut self) -> Result<(), ResourceError> {
        self.open = true;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmm_schema::fourcc;

    fn build_test_pak() -> Vec<u8> {
        let msg = b"HelloClockTown!\0"; // 16 bytes
        let id = ResourceId::from_otr_path("__OTR__text/test_msg").0;
        let entry = PmmPakEntry {
            id,
            fourcc: fourcc::OTXM,
            flags: PmmPakEntry::FLAG_PINNED_COMMON,
            offset: 112,
            size: msg.len() as u32,
            uncompressed_size: msg.len() as u32,
            align: 16,
        };
        let hdr = PmmPakHeader::new(1, 64, 112, 0x5445_5354, 0, 128);
        let mut buf = alloc::vec![0u8; 128];
        buf[0..64].copy_from_slice(&hdr.to_le_bytes());
        buf[64..96].copy_from_slice(&entry.to_le_bytes());
        buf[112..128].copy_from_slice(msg);
        buf
    }

    #[test]
    fn resource_mgr_lookup_hit_and_miss() {
        let mut mgr = ResourceManager::open(MemPakSource::new(build_test_pak())).unwrap();
        assert!(ResourceManager::<MemPakSource>::otr_sig_check("__OTR__text/test_msg"));
        assert!(!ResourceManager::<MemPakSource>::otr_sig_check("text/test_msg"));

        assert!(mgr.lookup_name("__OTR__text/test_msg").is_some());
        assert!(mgr.lookup_name("text/test_msg").is_some());
        assert!(mgr.lookup_name("__OTR__missing/resource").is_none());

        let mut raw = [0u8; 256];
        let mut arena = BoundedArena::from_slice(&mut raw);
        let (_, slice) = mgr
            .load_by_name_into_arena("__OTR__text/test_msg", &mut arena)
            .unwrap();
        assert_eq!(slice, b"HelloClockTown!\0");

        let err = mgr
            .load_by_name_into_arena("__OTR__missing/resource", &mut arena)
            .unwrap_err();
        assert_eq!(err, ResourceError::NotFound);
    }

    #[test]
    fn segment_table_disambiguation_and_oob() {
        let buf = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let mut segs = SegmentTable::new(0x0880_0000, 0x0C00_0000);
        segs.set_segment(0x02, &buf);

        let (ptr, rem) = segs.resolve(0x0200_0004).unwrap();
        assert_eq!(rem, 4);
        assert_eq!(unsafe { *ptr }, 0x55);

        assert_eq!(segs.resolve(0x0200_0020), Err(SegError::OutOfBounds));
        assert_eq!(segs.resolve(0), Err(SegError::NullAddress));
        assert_eq!(segs.resolve(0x0300_0000), Err(SegError::UnmappedSegment(3)));

        // Native PSP user-space pointer in 0x0880_0000..0x0C00_0000 when seg 0x08 inactive
        let (native_ptr, native_rem) = segs.resolve(0x0890_0000).unwrap();
        assert_eq!(native_ptr as usize, 0x0890_0000);
        assert_eq!(native_rem, 0x0C00_0000 - 0x0890_0000);
    }

    #[test]
    fn suspend_close_and_resume_reopen_contract() {
        let mut mgr = ResourceManager::open(MemPakSource::new(build_test_pak())).unwrap();
        let mut raw = [0u8; 256];
        let mut arena = BoundedArena::from_slice(&mut raw);

        mgr.suspend_close();
        assert!(!mgr.is_open());
        assert_eq!(
            mgr.load_by_name_into_arena("__OTR__text/test_msg", &mut arena)
                .unwrap_err(),
            ResourceError::FileClosed
        );

        mgr.resume_reopen().unwrap();
        assert!(mgr.is_open());
        let (_, slice) = mgr
            .load_by_name_into_arena("__OTR__text/test_msg", &mut arena)
            .unwrap();
        assert_eq!(slice, b"HelloClockTown!\0");
    }

    #[test]
    fn arena_scene_room_reset_zero_leak() {
        #[repr(C, align(16))]
        struct Aligned([u8; 512]);
        let mut raw = Aligned([0u8; 512]);
        let mut arena = BoundedArena::from_slice(&mut raw.0);
        for _ in 0..20 {
            arena.mark_scene();
            let a = arena.alloc_aligned(64, 16).unwrap();
            assert_eq!(a.as_ptr() as usize % 16, 0);
            arena.mark_room();
            let _ = arena.alloc_aligned(128, 32).unwrap();
            arena.reset_to_room();
            assert_eq!(arena.used_bytes(), 64);
            arena.reset_to_scene();
            assert_eq!(arena.used_bytes(), 0);
        }
    }

    #[test]
    fn truncated_pak_negative_test() {
        let full = build_test_pak();
        // Truncate payload
        let trunc = full[..120].to_vec();
        let mut mgr = ResourceManager::open(MemPakSource::new(trunc)).unwrap();
        let mut raw = [0u8; 256];
        let mut arena = BoundedArena::from_slice(&mut raw);
        assert_eq!(
            mgr.load_by_name_into_arena("__OTR__text/test_msg", &mut arena)
                .unwrap_err(),
            ResourceError::TruncatedRead
        );
        // Truncate header
        assert_eq!(
            ResourceManager::open(MemPakSource::new(full[..16].to_vec())).err(),
            Some(ResourceError::TruncatedRead)
        );
    }

    #[test]
    fn arena_oom_negative_test() {
        let mut raw = [0u8; 64];
        let mut arena = BoundedArena::from_slice(&mut raw);
        assert_eq!(
            arena.alloc_aligned(128, 16).unwrap_err(),
            ResourceError::OutOfMemory
        );
        assert_eq!(arena.used_bytes(), 0);
    }

    #[test]
    fn bad_pak_magic_negative_test() {
        let mut bad = build_test_pak();
        bad[0] = b'Z';
        assert_eq!(
            ResourceManager::open(MemPakSource::new(bad)).err(),
            Some(ResourceError::Schema(SchemaError::InvalidMagic))
        );
    }

    #[test]
    fn pinned_common_plus_20_scene_reload_cycles() {
        let mut mgr = ResourceManager::open(MemPakSource::new(build_test_pak())).unwrap();
        let mut common_raw = [0u8; 256];
        let mut scene_raw = [0u8; 256];
        let mut common = BoundedArena::from_slice(&mut common_raw);
        let mut scene = BoundedArena::from_slice(&mut scene_raw);

        let _ = mgr
            .load_by_name_into_arena("__OTR__text/test_msg", &mut common)
            .unwrap();
        let pinned_before = common.used_bytes();
        assert!(pinned_before > 0);

        for _ in 0..20 {
            scene.mark_scene();
            let (_, payload) = mgr
                .load_by_name_into_arena("__OTR__text/test_msg", &mut scene)
                .unwrap();
            assert_eq!(payload.as_ptr() as usize % 16, 0);
            assert_eq!(payload, b"HelloClockTown!\0");
            scene.reset_to_scene();
        }
        assert_eq!(scene.used_bytes(), 0);
        assert_eq!(common.used_bytes(), pinned_before);
    }
}
