//! What an upload sends: bytes in memory, or a range of a file that is read from disk while it is sent.

use std::{
    borrow::Cow,
    fs::File,
    io::{self, BufReader, Cursor, Read, Seek, SeekFrom},
    path::Path,
};

use sha2::digest::{Digest, Output};

/// Read buffer for file bodies: large enough that a recording does not turn into millions of tiny reads.
const FILE_BUFFER: usize = 256 * 1024;

/// The payload of an upload. `Copy`, so a chunked protocol hands out [`Body::slice`]s freely.
#[derive(Clone, Copy)]
pub enum Body<'a> {
    Bytes(&'a [u8]),
    /// `len` bytes of the file at `path`, from `offset`. The file is opened each time the body is read.
    File {
        path: &'a Path,
        offset: u64,
        len: u64,
    },
}

impl<'a> Body<'a> {
    /// The whole file at `path`, whose size is `len`.
    pub fn file(path: &'a Path, len: u64) -> Self {
        Body::File { path, offset: 0, len }
    }

    pub fn size(&self) -> u64 {
        match self {
            Body::Bytes(b) => b.len() as u64,
            Body::File { len, .. } => *len,
        }
    }

    /// The part of the body from `start` to `end` (exclusive, clamped to the size).
    pub fn slice(&self, start: u64, end: u64) -> Body<'a> {
        let (start, end) = (start.min(self.size()), end.min(self.size()));
        match *self {
            Body::Bytes(b) => Body::Bytes(&b[start as usize..end as usize]),
            Body::File { path, offset, .. } => Body::File { path, offset: offset + start, len: end - start },
        }
    }

    pub fn reader(&self) -> io::Result<Box<dyn Read + 'a>> {
        Ok(match *self {
            Body::Bytes(b) => Box::new(Cursor::new(b)),
            Body::File { path, offset, len } => {
                let mut file = File::open(path)?;
                file.seek(SeekFrom::Start(offset))?;
                Box::new(BufReader::with_capacity(FILE_BUFFER, file.take(len)))
            }
        })
    }

    /// The whole body in memory. For the few places that must look at all of it (a base64 template variable, text
    /// that is expanded into a template); everything that sends a body reads it through [`Body::reader`].
    pub fn bytes(&self) -> io::Result<Cow<'a, [u8]>> {
        match *self {
            Body::Bytes(b) => Ok(Cow::Borrowed(b)),
            Body::File { .. } => {
                let mut out = Vec::with_capacity(self.size() as usize);
                self.reader()?.read_to_end(&mut out)?;
                Ok(Cow::Owned(out))
            }
        }
    }

    /// Digest of the body, computed while streaming it.
    pub fn digest<D: Digest>(&self) -> io::Result<Output<D>> {
        let mut hasher = D::new();
        let mut reader = self.reader()?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                return Ok(hasher.finalize());
            }
            hasher.update(&buf[..n]);
        }
    }
}

/// One piece of a request body: bytes the request owns (headers, JSON) or payload that is only borrowed.
pub(crate) enum Chunk<'a> {
    Owned(Vec<u8>),
    Payload(Body<'a>),
}

impl<'a> Chunk<'a> {
    pub fn size(&self) -> u64 {
        match self {
            Chunk::Owned(v) => v.len() as u64,
            Chunk::Payload(b) => b.size(),
        }
    }

    pub fn reader<'r>(&'r self) -> io::Result<Box<dyn Read + 'r>>
    where
        'a: 'r,
    {
        match self {
            Chunk::Owned(v) => Ok(Box::new(Cursor::new(v.as_slice()))),
            Chunk::Payload(b) => b.reader(),
        }
    }
}
