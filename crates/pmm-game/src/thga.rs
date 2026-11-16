use alloc::vec;
use alloc::vec::Vec;
use pmm_schema::GfxWord;

pub const GFXPOOL_HEAD_MAGIC: u32 = 0x1234;
pub const GFXPOOL_TAIL_MAGIC: u32 = 0x5678;

/// 2S2H-derived Two-Head Graphics Arena (`THGA`): head grows upward (display list),
/// tail grows downward (matrices/vertices). Collision sets `is_crash() == true`.
pub struct TwoHeadGfxArena {
    buf: Vec<GfxWord>,
    head: usize,
    tail: usize,
}

impl TwoHeadGfxArena {
    pub fn new(capacity_words: usize) -> Self {
        Self {
            buf: vec![GfxWord::default(); capacity_words],
            head: 0,
            tail: capacity_words,
        }
    }

    pub fn reset(&mut self) {
        self.head = 0;
        self.tail = self.buf.len();
    }

    pub fn push_cmd(&mut self, cmd: GfxWord) -> bool {
        if self.head >= self.tail {
            return false;
        }
        self.buf[self.head] = cmd;
        self.head += 1;
        true
    }

    pub fn alloc_tail_words(&mut self, count: usize) -> Option<&mut [GfxWord]> {
        if self.tail < self.head + count {
            self.tail = self.head; // trigger crash canary
            return None;
        }
        self.tail -= count;
        Some(&mut self.buf[self.tail..self.tail + count])
    }

    pub fn is_crash(&self) -> bool {
        self.head >= self.tail
    }

    pub fn head_cmds(&self) -> &[GfxWord] {
        &self.buf[..self.head]
    }
}

pub struct GfxPool {
    pub head_magic: u32,
    pub poly_opa: TwoHeadGfxArena,
    pub poly_xlu: TwoHeadGfxArena,
    pub overlay: TwoHeadGfxArena,
    pub work: TwoHeadGfxArena,
    pub debug: TwoHeadGfxArena,
    pub tail_magic: u32,
}

impl Default for GfxPool {
    fn default() -> Self {
        Self::new()
    }
}

impl GfxPool {
    pub fn new() -> Self {
        Self {
            head_magic: GFXPOOL_HEAD_MAGIC,
            poly_opa: TwoHeadGfxArena::new(2048),
            poly_xlu: TwoHeadGfxArena::new(1024),
            overlay: TwoHeadGfxArena::new(1024),
            work: TwoHeadGfxArena::new(512),
            debug: TwoHeadGfxArena::new(256),
            tail_magic: GFXPOOL_TAIL_MAGIC,
        }
    }

    pub fn reset(&mut self) {
        self.head_magic = GFXPOOL_HEAD_MAGIC;
        self.tail_magic = GFXPOOL_TAIL_MAGIC;
        self.poly_opa.reset();
        self.poly_xlu.reset();
        self.overlay.reset();
        self.work.reset();
        self.debug.reset();
    }

    pub fn check_canaries_and_crash(&self) -> bool {
        self.head_magic != GFXPOOL_HEAD_MAGIC
            || self.tail_magic != GFXPOOL_TAIL_MAGIC
            || self.poly_opa.is_crash()
            || self.poly_xlu.is_crash()
            || self.overlay.is_crash()
            || self.work.is_crash()
            || self.debug.is_crash()
    }
}
