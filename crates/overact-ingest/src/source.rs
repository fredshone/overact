//! Random-access byte sources: a file on disk, a buffer, or (in the browser)
//! a `File` read synchronously by the host.

use std::io;
use std::sync::Mutex;

/// A source of bytes that can be read at any offset.
///
/// `Send + Sync` because the Parquet reader requires it; hosts with a
/// single-threaded source (the browser) only ever read from one thread.
pub trait ByteSource: Send + Sync {
    fn len(&self) -> u64;
    /// Fills `buf` with the bytes at `offset`. Errors if the range runs past the end.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ByteSource for Vec<u8> {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let a = offset as usize;
        let src = self
            .get(a..a + buf.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "read past end"))?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// A file on disk.
pub struct FileSource {
    file: Mutex<std::fs::File>,
    len: u64,
}

impl FileSource {
    pub fn open(path: impl AsRef<std::path::Path>) -> io::Result<FileSource> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(FileSource {
            file: Mutex::new(file),
            len,
        })
    }
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)
    }
}
