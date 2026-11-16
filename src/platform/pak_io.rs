use alloc::string::String;
use pmm_assets::{PakSource, ResourceError};
use psp::io::File;
use psp::sys::{IoOpenFlags, IoWhence, sceKernelDelayThread};

const IO_CHUNK_BYTES: usize = 0x4000; // 16 KiB
const ZERO_READ_RETRIES: usize = 16;

/// PSP `sceIo*` pak reader with 16 KiB chunked reads, retry handling,
/// and suspend/resume FD lifecycle (`suspend_close` / `resume_reopen`).
pub struct PspPakSource {
    path: String,
    file: Option<File>,
}

impl PspPakSource {
    pub fn open(path: &str) -> Result<Self, ResourceError> {
        let file = File::open(path, IoOpenFlags::RD_ONLY).map_err(|e| ResourceError::Io(e.code()))?;
        Ok(Self {
            path: String::from(path),
            file: Some(file),
        })
    }
}

impl PakSource for PspPakSource {
    fn read_at(&mut self, offset: u32, dst: &mut [u8]) -> Result<(), ResourceError> {
        let file = self.file.as_ref().ok_or(ResourceError::FileClosed)?;
        file.seek(offset as i64, IoWhence::Set)
            .map_err(|e| ResourceError::Io(e.code()))?;

        let mut total = 0usize;
        let mut zero_retries = 0usize;
        while total < dst.len() {
            let chunk = (dst.len() - total).min(IO_CHUNK_BYTES);
            match file.read(&mut dst[total..total + chunk]) {
                Ok(0) => {
                    zero_retries += 1;
                    if zero_retries >= ZERO_READ_RETRIES {
                        return Err(ResourceError::TruncatedRead);
                    }
                    unsafe {
                        sceKernelDelayThread(1000);
                    }
                }
                Ok(n) => {
                    zero_retries = 0;
                    total += n;
                }
                Err(e) => return Err(ResourceError::Io(e.code())),
            }
        }
        Ok(())
    }

    fn suspend_close(&mut self) {
        self.file = None; // Drops `File`, closing `SceUID` before sleep
    }

    fn resume_reopen(&mut self) -> Result<(), ResourceError> {
        if self.file.is_none() {
            let file =
                File::open(&self.path, IoOpenFlags::RD_ONLY).map_err(|e| ResourceError::Io(e.code()))?;
            self.file = Some(file);
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.file.is_some()
    }
}
