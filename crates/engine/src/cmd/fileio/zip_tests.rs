use super::*;
use std::io::{Cursor, Read};

#[test]
fn max_files_round_trip_through_the_reader() {
    let at_limit: Vec<(String, &[u8])> = (0..65534).map(|index| (format!("{index}"), &[][..])).collect();
    assert!(Archive::open(&store(&at_limit).unwrap(), usize::MAX).is_ok());
    assert!(Archive::open(&deflate(&at_limit, FIXED_TIME, || false).unwrap(), usize::MAX).is_ok());
    let over: Vec<(String, &[u8])> = (0..65535).map(|index| (format!("{index}"), &[][..])).collect();
    assert!(store(&over).is_err());
    assert!(deflate(&over, FIXED_TIME, || false).is_err());
}

fn records(bytes: &[u8]) -> Vec<(String, u32, u64, u64, zip::CompressionMethod)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|index| {
            let file = archive.by_index(index).unwrap();
            (file.name().unwrap().into_owned(), file.crc32(), file.size(), file.compressed_size(), file.compression())
        })
        .collect()
}

fn check_pair(files: &[(String, Vec<u8>)], compressed: bool) -> bool {
    let old = if compressed { legacy::deflate(files, FIXED_TIME, || false).unwrap() } else { legacy::store(files).unwrap() };
    let new = if compressed { deflate(files, FIXED_TIME, || false).unwrap() } else { store(files).unwrap() };
    assert_eq!(entries(&new).unwrap(), entries(&old).unwrap());
    assert_eq!(records(&new), records(&old));
    assert_eq!(&new[new.len() - 10..new.len() - 6], &old[old.len() - 10..old.len() - 6]);
    if files.iter().any(|(name, _)| name == "noise.bin") {
        assert_eq!(records(&new)[0].4, zip::CompressionMethod::STORE);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(&new)).unwrap();
    for (index, (name, expected)) in files.iter().enumerate() {
        let mut file = archive.by_index(index).unwrap();
        assert_eq!(file.name().unwrap(), name.as_str());
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(&bytes, expected);
    }
    old == new
}

#[test]
fn old_and_shared_writers_preserve_entries_and_central_records() {
    let mut state = 0x9E37_79B9u32;
    let noise: Vec<u8> = (0..4096)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let cases = [
        (vec![], false),
        (vec![("stored.txt".into(), b"stored".to_vec())], false),
        (vec![("compressed.txt".into(), vec![b'a'; 4096])], true),
        (vec![("a.txt".into(), vec![b'a'; 4096]), ("b.bin".into(), noise.clone())], true),
        (vec![("caf\u{e9}.txt".into(), b"unicode".to_vec())], true),
        (vec![("noise.bin".into(), noise)], true),
    ];
    let identical: Vec<bool> = cases.iter().map(|(files, compressed)| check_pair(files, *compressed)).collect();
    eprintln!("old/new ZIP byte-identical cases: {identical:?}");
}

#[test]
fn hostile_archives_fail_without_panicking() {
    let mut bomb = store(&[("bomb", b"ok")]).unwrap();
    let central = bomb.windows(4).position(|window| window == 0x0201_4b50u32.to_le_bytes()).unwrap();
    bomb[central + 24..central + 28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Archive::open(&bomb, 10).is_err());

    let unsafe_name = legacy::store(&[("../escape", b"bad")]).unwrap();
    assert!(Archive::open(&unsafe_name, 10).is_err());

    let mut truncated = store(&[("name", b"body")]).unwrap();
    truncated.truncate(truncated.len() - 7);
    assert!(Archive::open(&truncated, 10).is_err());

    let normal = store(&[("ok", vec![b'x'; 2048])]).unwrap();
    let archive = Archive::open(&normal, 10).unwrap();
    assert_eq!(archive.read("missing", 0).unwrap(), None);
    assert_eq!(archive.read("OK", 2047), Err(ReadError::TooLarge("ok".into())));
    assert_eq!(archive.read("OK", 2048).unwrap(), Some(vec![b'x'; 2048]));
}

#[test]
fn small_entry_reads_when_another_entry_is_larger_than_its_limit() {
    let bytes = store(&[("small", vec![1]), ("large", vec![2; 8192])]).unwrap();
    let archive = Archive::open(&bytes, 2).unwrap();
    assert_eq!(archive.read("SMALL", 1).unwrap(), Some(vec![1]));
    assert_eq!(archive.read("large", 1), Err(ReadError::TooLarge("large".into())));
}

/// Rewrite the compression method of the only entry, in its local and central headers.
fn with_method(mut bytes: Vec<u8>, method: u16) -> Vec<u8> {
    let at = |bytes: &[u8], signature: u32| bytes.windows(4).position(|window| window == signature.to_le_bytes()).unwrap();
    let (local, central) = (at(&bytes, 0x0403_4b50), at(&bytes, 0x0201_4b50));
    bytes[local + 8..local + 10].copy_from_slice(&method.to_le_bytes());
    bytes[central + 10..central + 12].copy_from_slice(&method.to_le_bytes());
    bytes
}

#[test]
fn an_unsupported_compression_method_is_a_method_error_not_damage() {
    // Every method the pre-swap reader refused: all but stored (0) and deflated (8).
    for method in (0..=u16::MAX).filter(|method| ![0, 8].contains(method)) {
        let bytes = with_method(store(&[("data", vec![7; 64])]).unwrap(), method);
        let archive = Archive::open(&bytes, 10).unwrap_or_else(|error| panic!("method {method}: open failed with {error:?}"));
        assert_eq!(archive.read("DATA", 1 << 20), Err(ReadError::Method("data".into(), method)), "method {method}");
    }
}

#[test]
fn an_oversize_entry_is_too_large_before_its_method_is_considered() {
    // The pre-swap reader checked the size cap before the method.
    let bytes = with_method(store(&[("data", vec![7; 64])]).unwrap(), 12);
    let archive = Archive::open(&bytes, 10).unwrap();
    assert_eq!(archive.read("data", 8), Err(ReadError::TooLarge("data".into())));
}
