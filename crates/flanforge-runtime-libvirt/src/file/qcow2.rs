use crate::RuntimeError;

const QCOW2_MAGIC: [u8; 4] = [b'Q', b'F', b'I', 0xfb];
const HEADER_BYTES: usize = 32;
/// Backing filenames live in the first cluster in every image qemu writes, and
/// a declared string outside this window is reported as unreadable rather than
/// guessed at.
pub(crate) const HEADER_WINDOW_BYTES: u64 = 64 * 1_024;

/// What a qcow2 file says about itself, read from its own bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Qcow2Header {
    pub(crate) virtual_bytes: u64,
    pub(crate) backing_file: Option<String>,
}

/// Reads the qcow2 header out of a prefix of a file.
///
/// `Ok(None)` means the bytes are not qcow2 at all — a raw volume has no
/// backing file, and that is a definitive answer rather than an unreadable
/// one. An error means the file claims a backing filename this window cannot
/// show, which callers must treat as inconclusive.
pub(crate) fn parse_header(bytes: &[u8]) -> Result<Option<Qcow2Header>, RuntimeError> {
    if bytes.len() < HEADER_BYTES || bytes[..4] != QCOW2_MAGIC {
        return Ok(None);
    }
    let version = read_u32(bytes, 4);
    if version < 2 {
        return Ok(None);
    }
    let backing_offset = read_u64(bytes, 8);
    let backing_size = read_u32(bytes, 16) as usize;
    let virtual_bytes = read_u64(bytes, 24);
    if backing_offset == 0 || backing_size == 0 {
        return Ok(Some(Qcow2Header {
            virtual_bytes,
            backing_file: None,
        }));
    }
    if backing_size > 4_096 {
        return Err(RuntimeError::manifest(
            "qcow2 backing filename length is out of range",
        ));
    }
    let start = usize::try_from(backing_offset)
        .map_err(|_| RuntimeError::manifest("qcow2 backing filename offset is out of range"))?;
    let end = start
        .checked_add(backing_size)
        .ok_or_else(|| RuntimeError::manifest("qcow2 backing filename overflows"))?;
    let name = bytes
        .get(start..end)
        .ok_or_else(|| RuntimeError::manifest("qcow2 backing filename is outside the header"))?;
    let name = std::str::from_utf8(name)
        .map_err(|_| RuntimeError::manifest("qcow2 backing filename is not UTF-8"))?;
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(RuntimeError::manifest("qcow2 backing filename is unsafe"));
    }
    Ok(Some(Qcow2Header {
        virtual_bytes,
        backing_file: Some(name.to_owned()),
    }))
}

/// Lexically normalizes an absolute path so two spellings of one file compare
/// equal without a stat the daemon may not be able to perform.
pub(crate) fn normalize_path(value: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in value.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    if value.starts_with('/') {
        format!("/{}", segments.join("/"))
    } else {
        segments.join("/")
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut value = [0_u8; 4];
    value.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_be_bytes(value)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut value = [0_u8; 8];
    value.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_be_bytes(value)
}
