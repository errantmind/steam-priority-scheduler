#![forbid(unsafe_code)]

use crate::sys::Result;
pub struct Entry<'a> {
    /// Checked child name, including its terminating NUL.
    pub name: &'a [u8],
    pub kind: u8,
    /// Kernel continuation position; used only when a traversal shares its buffer.
    pub cookie: i64,
}

/// Decode one Linux x86-64 getdents64 record without structure casts.
/// The cursor changes only after the complete record has passed validation.
pub fn next<'a>(data: &'a [u8], cursor: &mut usize) -> Result<Entry<'a>> {
    let invalid = "Invalid directory record.";
    let tail = data.get(*cursor..).ok_or(invalid)?;
    let header = tail.get(..19).ok_or(invalid)?;
    let length = u16::from_ne_bytes([header[16], header[17]]) as usize;
    // The kernel aligns records to sizeof(long), including the terminating NUL.
    if length < 24 || length % 8 != 0 {
        return Err(invalid);
    }
    let record = tail.get(..length).ok_or(invalid)?;
    for (index, &byte) in record[19..].iter().enumerate() {
        if byte == b'/' {
            return Err(invalid);
        }
        if byte == 0 {
            if index == 0 {
                return Err(invalid);
            }
            *cursor += length;
            return Ok(Entry {
                name: &record[19..20 + index],
                kind: header[18],
                cookie: i64::from_ne_bytes(header[8..16].try_into().map_err(|_| invalid)?),
            });
        }
    }
    Err(invalid)
}
