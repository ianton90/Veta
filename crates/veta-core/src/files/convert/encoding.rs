//! Text encodings: reading Latin-1, Windows-1252 and UTF-16 input as UTF-8.

use super::*;

/// Text encodings for CSV files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEncoding {
    /// UTF-8; files starting with a UTF-16 byte order mark are read as
    /// UTF-16.
    #[default]
    Utf8,
    /// ISO-8859-1.
    Latin1,
    /// Windows-1252, Windows' "ANSI" for Western languages (Latin-1 plus
    /// characters such as € and curly quotes).
    Windows1252,
    Utf16Le,
    Utf16Be,
}

impl TextEncoding {
    pub const ALL: [TextEncoding; 5] = [
        Self::Utf8,
        Self::Latin1,
        Self::Windows1252,
        Self::Utf16Le,
        Self::Utf16Be,
    ];
}

impl std::fmt::Display for TextEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Utf8 => "UTF-8",
            Self::Latin1 => "Latin-1 (ISO-8859-1)",
            Self::Windows1252 => "Windows-1252",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
        })
    }
}

/// Windows-1252 characters for bytes 0x80-0x9F (the rest match Latin-1).
pub(super) const WINDOWS_1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

// Reading text.

/// Reads a file as UTF-8, transcoding Latin-1 and dropping a UTF-8 byte
/// order mark; counts bytes and stops when `progress` says to.
pub(super) struct Input<'a> {
    inner: BufReader<File>,
    encoding: TextEncoding,
    read: u64,
    total: u64,
    progress: Option<&'a mut dyn FnMut(f32) -> bool>,
    pub(super) cancelled: bool,
    /// Transcoded bytes not yet returned.
    pending: Vec<u8>,
    /// UTF-16 bytes kept for the next chunk (an odd byte, or the first
    /// half of a surrogate pair).
    carry: Vec<u8>,
    started: bool,
}

impl<'a> Input<'a> {
    pub(super) fn open(
        path: &Path,
        encoding: TextEncoding,
        progress: Option<&'a mut dyn FnMut(f32) -> bool>,
    ) -> Result<Self> {
        let file = File::open(path)?;
        let total = file.metadata()?.len();
        Ok(Self {
            inner: BufReader::with_capacity(1 << 16, file),
            encoding,
            read: 0,
            total,
            progress,
            cancelled: false,
            pending: Vec::new(),
            carry: Vec::new(),
            started: false,
        })
    }
}

impl Read for Input<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.pending.is_empty() {
            let n = buf.len().min(self.pending.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            return Ok(n);
        }
        let chunk = self.inner.fill_buf()?;
        let mut chunk = chunk.to_vec();
        let len = chunk.len();
        self.inner.consume(len);
        if len == 0 {
            if self.carry.is_empty() {
                return Ok(0);
            }
            // A dangling UTF-16 byte or surrogate at the end.
            let rest = std::mem::take(&mut self.carry);
            self.pending = decode_utf16(&rest, self.encoding == TextEncoding::Utf16Be, true).0;
            return self.read(buf);
        }
        self.read += len as u64;
        if let Some(progress) = &mut self.progress
            && !progress(self.read as f32 / self.total.max(1) as f32)
        {
            self.cancelled = true;
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if !self.started {
            self.started = true;
            if self.encoding == TextEncoding::Utf8 && chunk.starts_with(&[0xEF, 0xBB, 0xBF]) {
                chunk.drain(..3);
            }
            // A UTF-16 byte order mark picks the byte order.
            let utf16 = matches!(
                self.encoding,
                TextEncoding::Utf8 | TextEncoding::Utf16Le | TextEncoding::Utf16Be
            );
            if utf16 && chunk.starts_with(&[0xFF, 0xFE]) {
                self.encoding = TextEncoding::Utf16Le;
                chunk.drain(..2);
            } else if utf16 && chunk.starts_with(&[0xFE, 0xFF]) {
                self.encoding = TextEncoding::Utf16Be;
                chunk.drain(..2);
            }
        }
        self.pending = match self.encoding {
            TextEncoding::Utf8 => chunk,
            TextEncoding::Latin1 => chunk
                .iter()
                .map(|&b| b as char)
                .collect::<String>()
                .into_bytes(),
            TextEncoding::Windows1252 => chunk
                .iter()
                .map(|&b| match b {
                    0x80..=0x9F => WINDOWS_1252[usize::from(b - 0x80)],
                    _ => b as char,
                })
                .collect::<String>()
                .into_bytes(),
            TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
                let mut bytes = std::mem::take(&mut self.carry);
                bytes.extend_from_slice(&chunk);
                let (text, rest) =
                    decode_utf16(&bytes, self.encoding == TextEncoding::Utf16Be, false);
                self.carry = rest;
                text
            }
        };
        self.read(buf)
    }
}

/// Decodes UTF-16 `bytes` to UTF-8, returning the bytes to keep for the
/// next chunk: an odd last byte, or a high surrogate waiting for its pair
/// (`at_end`: there is no next chunk).
/// Invalid units become U+FFFD.
pub(super) fn decode_utf16(bytes: &[u8], big_endian: bool, at_end: bool) -> (Vec<u8>, Vec<u8>) {
    let even = bytes.len() - bytes.len() % 2;
    let mut units: Vec<u16> = bytes[..even]
        .chunks_exact(2)
        .map(|p| {
            if big_endian {
                u16::from_be_bytes([p[0], p[1]])
            } else {
                u16::from_le_bytes([p[0], p[1]])
            }
        })
        .collect();
    let mut rest = bytes[even..].to_vec();
    if !at_end && units.last().is_some_and(|u| (0xD800..0xDC00).contains(u)) {
        let unit = units.pop().unwrap_or_default();
        let pair = if big_endian {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        };
        rest.splice(0..0, pair);
    }
    let text: String = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    (text.into_bytes(), rest)
}

/// Turns a JSON array of objects into one object per line.
pub(super) struct ArrayToLines<R> {
    inner: R,
    /// Inside the outer `[`.
    opened: bool,
    depth: usize,
    in_string: bool,
    escaped: bool,
    pending: Vec<u8>,
}

impl<R: Read> ArrayToLines<R> {
    pub(super) fn new(inner: R) -> Self {
        Self {
            inner,
            opened: false,
            depth: 0,
            in_string: false,
            escaped: false,
            pending: Vec::new(),
        }
    }
}

impl<R: Read> Read for ArrayToLines<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() {
            let mut chunk = [0u8; 1 << 16];
            let n = self.inner.read(&mut chunk)?;
            if n == 0 {
                return Ok(0);
            }
            for &b in &chunk[..n] {
                if self.depth == 0 {
                    match b {
                        b'[' if !self.opened => self.opened = true,
                        b'{' if self.opened => {
                            self.depth = 1;
                            self.pending.push(b);
                        }
                        b',' | b']' | b' ' | b'\t' | b'\r' | b'\n' if self.opened => {}
                        b' ' | b'\t' | b'\r' | b'\n' | 0xEF | 0xBB | 0xBF => {}
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "expected a JSON array of objects",
                            ));
                        }
                    }
                    continue;
                }
                self.pending.push(b);
                if self.in_string {
                    match (self.escaped, b) {
                        (true, _) => self.escaped = false,
                        (false, b'\\') => self.escaped = true,
                        (false, b'"') => self.in_string = false,
                        _ => {}
                    }
                    continue;
                }
                match b {
                    b'"' => self.in_string = true,
                    b'{' | b'[' => self.depth += 1,
                    b'}' | b']' => {
                        self.depth -= 1;
                        if self.depth == 0 {
                            self.pending.push(b'\n');
                        }
                    }
                    _ => {}
                }
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}
