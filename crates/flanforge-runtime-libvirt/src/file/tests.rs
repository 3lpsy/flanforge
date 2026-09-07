use std::os::unix::fs::symlink;

use super::read_bounded_regular;

#[test]
fn bounded_reader_rejects_symlinks_and_oversized_files() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let target = directory.path().join("target");
    let link = directory.path().join("link");
    std::fs::write(&target, b"12345").unwrap_or_else(|error| unreachable!("target: {error}"));
    symlink(&target, &link).unwrap_or_else(|error| unreachable!("link: {error}"));
    assert!(read_bounded_regular(&link, 16, "fixture").is_err());
    assert!(read_bounded_regular(&target, 4, "fixture").is_err());
    assert_eq!(
        read_bounded_regular(&target, 5, "fixture"),
        Ok(b"12345".to_vec())
    );
}

use super::{Qcow2Header, normalize_path, parse_header};

/// A qcow2 header with an optional backing filename, big-endian as on disk.
fn qcow2(virtual_bytes: u64, backing: Option<&str>) -> Vec<u8> {
    let mut bytes = vec![0_u8; 512];
    bytes[..4].copy_from_slice(&[b'Q', b'F', b'I', 0xfb]);
    bytes[4..8].copy_from_slice(&3_u32.to_be_bytes());
    bytes[24..32].copy_from_slice(&virtual_bytes.to_be_bytes());
    if let Some(backing) = backing {
        let offset = 128_u64;
        bytes[8..16].copy_from_slice(&offset.to_be_bytes());
        let length = u32::try_from(backing.len()).unwrap_or_else(|error| unreachable!("{error}"));
        bytes[16..20].copy_from_slice(&length.to_be_bytes());
        let start = usize::try_from(offset).unwrap_or_else(|error| unreachable!("{error}"));
        bytes[start..start + backing.len()].copy_from_slice(backing.as_bytes());
    }
    bytes
}

/// The header is the authoritative backing-file instrument: it is the file.
#[test]
fn a_qcow2_header_reports_its_own_backing_file() {
    assert_eq!(
        parse_header(&qcow2(40, None)),
        Ok(Some(Qcow2Header {
            virtual_bytes: 40,
            backing_file: None,
        }))
    );
    assert_eq!(
        parse_header(&qcow2(40, Some("/pool/base.qcow2"))),
        Ok(Some(Qcow2Header {
            virtual_bytes: 40,
            backing_file: Some("/pool/base.qcow2".to_owned()),
        }))
    );
}

/// A raw volume is not qcow2 and definitively has no backing file; anything
/// truncated is unreadable rather than unbacked.
#[test]
fn a_non_qcow2_prefix_is_a_definitive_answer_and_a_truncated_one_is_not() {
    assert_eq!(parse_header(b"not a qcow2 image at all........."), Ok(None));
    assert_eq!(parse_header(b"QFI"), Ok(None));
    let mut truncated = qcow2(40, Some("/pool/base.qcow2"));
    truncated.truncate(64);
    assert!(
        parse_header(&truncated).is_err(),
        "a declared name this window cannot show must be inconclusive"
    );
}

#[test]
fn a_backing_filename_outside_the_window_is_refused() {
    let mut bytes = qcow2(40, None);
    bytes[8..16].copy_from_slice(&(1_u64 << 40).to_be_bytes());
    bytes[16..20].copy_from_slice(&16_u32.to_be_bytes());
    assert!(parse_header(&bytes).is_err());
}

#[test]
fn path_normalization_makes_two_spellings_of_one_file_compare_equal() {
    assert_eq!(normalize_path("/pool//base.qcow2"), "/pool/base.qcow2");
    assert_eq!(normalize_path("/pool/./base.qcow2"), "/pool/base.qcow2");
    assert_eq!(
        normalize_path("/pool/sub/../base.qcow2"),
        "/pool/base.qcow2"
    );
    assert_eq!(normalize_path("/pool/base.qcow2/"), "/pool/base.qcow2");
}
