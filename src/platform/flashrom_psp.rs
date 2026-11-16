use pmm_game::save::{FLASH_TOTAL_BYTES, FlashIo};
use psp::io::File;
use psp::sys::{self, IoOpenFlags};

const SAVE_PATH: &str = "mm-psp-save.bin";
const SAVE_TMP_PATH: &str = "mm-psp-save.tmp";
const IO_CHUNK: usize = 0x4000;

pub struct PspFlashIo;

impl FlashIo for PspFlashIo {
    fn load_image(&mut self, buf: &mut [u8]) -> Result<(), i32> {
        let f = File::open(SAVE_PATH, IoOpenFlags::RD_ONLY).map_err(|e| e.code())?;
        let mut off = 0usize;
        while off < buf.len() {
            let chunk = (buf.len() - off).min(IO_CHUNK);
            match f.read(&mut buf[off..off + chunk]) {
                Ok(0) => break,
                Ok(n) => off += n,
                Err(e) => return Err(e.code()),
            }
        }
        Ok(())
    }

    fn flush_image(&mut self, buf: &[u8]) -> Result<(), i32> {
        assert_eq!(buf.len(), FLASH_TOTAL_BYTES);
        {
            let f = File::create(SAVE_TMP_PATH).map_err(|e| e.code())?;
            let mut off = 0usize;
            while off < buf.len() {
                let chunk = (buf.len() - off).min(IO_CHUNK);
                match f.write(&buf[off..off + chunk]) {
                    Ok(0) => return Err(-1),
                    Ok(n) => off += n,
                    Err(e) => return Err(e.code()),
                }
            }
        }
        // Atomic replace on ms0:/ef0:
        let _ = unsafe { sys::sceIoRemove(b"mm-psp-save.bin\0".as_ptr()) };
        let r = unsafe {
            sys::sceIoRename(
                b"mm-psp-save.tmp\0".as_ptr(),
                b"mm-psp-save.bin\0".as_ptr(),
            )
        };
        if r < 0 { Err(r) } else { Ok(()) }
    }
}
