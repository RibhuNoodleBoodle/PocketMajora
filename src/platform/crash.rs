use psp::io::File;

/// Write text completely to a file relative to the EBOOT launch directory.
pub fn write_text_file(path: &str, contents: &str) -> Result<(), i32> {
    let f = File::create(path).map_err(|e| e.code())?;
    let bytes = contents.as_bytes();
    let mut off = 0usize;
    while off < bytes.len() {
        match f.write(&bytes[off..]) {
            Ok(0) => return Err(-1),
            Ok(n) => off += n,
            Err(e) => return Err(e.code()),
        }
    }
    Ok(())
}
