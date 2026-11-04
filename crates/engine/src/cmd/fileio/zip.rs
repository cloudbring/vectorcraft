//! ZIP archives for exported files and guarded, bounded ZIP reads.

use craft_archive::{BoundedInflate, DosTime, InflateError, Limits, Wrapper, ZipError, ZipLimits, ZipReader, ZipWriteLimits, ZipWriter};
use std::cell::RefCell;

/// The MS-DOS time and date of every stored entry: midnight, 1980-01-01.
pub const FIXED_TIME: (u16, u16) = (0, (1 << 5) | 1);
/// Bytes compressed between cancellation checks.
pub const DEFLATE_CHUNK: usize = 1 << 20;
const MAX_READ_ENTRY: usize = 512 * 1024 * 1024;

fn write_limits() -> ZipWriteLimits {
    ZipWriteLimits { max_entries: 65534, max_entry_bytes: u32::MAX as u64 - 1, max_total_bytes: u32::MAX as u64 - 1 }
}

/// Write the files without compression, within the classic ZIP limits.
pub fn store<N: AsRef<str>, B: AsRef<[u8]>>(files: &[(N, B)]) -> Result<Vec<u8>, String> {
    let mut writer = ZipWriter::with_limits(std::io::Cursor::new(Vec::new()), write_limits());
    writer.set_time(DosTime { time: FIXED_TIME.0, date: FIXED_TIME.1 });
    for (name, bytes) in files {
        writer.add_stored(name.as_ref(), bytes.as_ref()).map_err(|error| error.to_string())?;
    }
    writer.finish().map(|cursor| cursor.into_inner()).map_err(|error| error.to_string())
}

/// Deflate each file if smaller; abandon the archive when `stop` requests cancellation.
pub fn deflate<N: AsRef<str>, B: AsRef<[u8]>>(files: &[(N, B)], (time, date): (u16, u16), stop: impl Fn() -> bool) -> Result<Vec<u8>, String> {
    let mut writer = ZipWriter::with_limits(std::io::Cursor::new(Vec::new()), write_limits());
    writer.set_time(DosTime { time, date });
    for (name, bytes) in files {
        writer.add_deflated_if_smaller_with_stop(name.as_ref(), bytes.as_ref(), 6, &stop).map_err(|error| error.to_string())?;
    }
    writer.finish().map(|cursor| cursor.into_inner()).map_err(|error| error.to_string())
}

/// Does `bytes` start with a ZIP archive's first local file header?
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(&0x0403_4b50u32.to_le_bytes())
}

/// Why [`Archive`] can't read an archive or one of its files.
#[derive(Debug, PartialEq)]
pub enum ReadError {
    NotZip,
    TooManyFiles,
    Zip64,
    Encrypted(String),
    Method(String, u16),
    TooLarge(String),
    Damaged(String),
}

fn read_limits(max_files: usize, max_entry_bytes: u64) -> ZipLimits {
    ZipLimits { max_entries: max_files, max_entry_bytes, max_total_bytes: 2 * 1024 * 1024 * 1024 }
}

fn read_error(error: ZipError, name: &str) -> ReadError {
    match error {
        ZipError::EntryCount => ReadError::TooManyFiles,
        ZipError::EntryLimit | ZipError::TotalLimit => ReadError::TooLarge(name.to_owned()),
        ZipError::Archive(zip::result::ZipError::CompressionMethodNotSupported(method)) => ReadError::Method(name.to_owned(), method),
        ZipError::Archive(zip::result::ZipError::UnsupportedArchive(zip::result::ZipError::PASSWORD_REQUIRED))
        | ZipError::Archive(zip::result::ZipError::InvalidPassword) => ReadError::Encrypted(name.to_owned()),
        ZipError::UnsafePath(path) | ZipError::Symlink(path) => ReadError::Damaged(path),
        _ => ReadError::Damaged(name.to_owned()),
    }
}

fn understated_deflate_exceeds_limit(zip: &[u8], name: &str, max: usize) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(zip)) else { return false };
    let Ok(file) = archive.by_name(name) else { return false };
    if file.compression() != zip::CompressionMethod::DEFLATE {
        return false;
    }
    let Some(start) = file.data_start().and_then(|offset| usize::try_from(offset).ok()) else { return false };
    let Some(end) = usize::try_from(file.compressed_size()).ok().and_then(|size| start.checked_add(size)) else { return false };
    let Some(data) = zip.get(start..end) else { return false };
    let limits = Limits { max_output: max as u64, max_input: data.len() as u64, max_ratio: u64::MAX, wrapper: Wrapper::Raw };
    let Ok(mut inflater) = BoundedInflate::new(data, limits) else { return false };
    let mut buffer = [0; 8192];
    loop {
        match inflater.read_chunk(&mut buffer) {
            Err(InflateError::OutputLimit { .. }) => return true,
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
    }
}

/// Validated ZIP entries, retained in central-directory order. One validated reader serves every
/// read: opening checks all names and sizes, so a read must not repeat that work.
pub struct Archive<'a> {
    zip: &'a [u8],
    reader: RefCell<ZipReader<'a>>,
    max_files: usize,
}

impl<'a> Archive<'a> {
    /// Open a ZIP containing at most `max_files` entries.
    pub fn open(zip: &'a [u8], max_files: usize) -> Result<Self, ReadError> {
        let reader = Self::validated(zip, max_files).map_err(|error| {
            if !is_zip(zip) && !zip.starts_with(&0x0605_4b50u32.to_le_bytes()) { ReadError::NotZip } else { read_error(error, "") }
        })?;
        Ok(Self { zip, reader: RefCell::new(reader), max_files })
    }

    fn validated(zip: &'a [u8], max_files: usize) -> Result<ZipReader<'a>, ZipError> {
        ZipReader::open(zip, read_limits(max_files, MAX_READ_ENTRY as u64))
    }

    /// Check for an entry in any ASCII case.
    pub fn contains(&self, name: &str) -> bool {
        self.reader.borrow().names().iter().any(|entry| entry.eq_ignore_ascii_case(name))
    }

    /// Read an entry in any ASCII case, never decompressing more than `max` bytes.
    pub fn read(&self, name: &str, max: usize) -> Result<Option<Vec<u8>>, ReadError> {
        let max = max.min(MAX_READ_ENTRY);
        let mut reader = self.reader.borrow_mut();
        let Some(entry) = reader.names().iter().find(|entry| entry.eq_ignore_ascii_case(name)).cloned() else { return Ok(None) };
        // The pre-swap reader's order: encryption, then the size cap, then the method.
        if let Some(info) = reader.entry(&entry) {
            if info.encrypted {
                return Err(ReadError::Encrypted(entry));
            }
            if info.size > max as u64 {
                return Err(ReadError::TooLarge(entry));
            }
            if !matches!(info.compression_method, 0 | 8) {
                return Err(ReadError::Method(entry, info.compression_method));
            }
        }
        let mut result = reader.read_bounded(&entry, max as u64);
        if matches!(result, Err(ZipError::TotalLimit)) {
            // The reader counts every byte it has returned; each read here is capped on its own, so
            // start a fresh count rather than let a long-lived archive run out.
            if let Ok(fresh) = Self::validated(self.zip, self.max_files) {
                *reader = fresh;
                result = reader.read_bounded(&entry, max as u64);
            }
        }
        result.map_err(|error| {
            if understated_deflate_exceeds_limit(self.zip, &entry, max) { ReadError::TooLarge(entry.clone()) } else { read_error(error, &entry) }
        })
    }
}

/// Decode every entry of a test archive, checking CRCs and decompressed sizes.
#[cfg(test)]
pub fn entries(zip: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let archive = Archive::open(zip, 65534).map_err(|error| format!("{error:?}"))?;
    let names = archive.reader.borrow().names().to_vec();
    names
        .iter()
        .map(|name| {
            archive.read(name, 512 * 1024 * 1024).map(|bytes| (name.clone(), bytes.unwrap_or_default())).map_err(|error| format!("{error:?}"))
        })
        .collect()
}

#[cfg(test)]
#[path = "zip_legacy_test.rs"]
mod legacy;

#[cfg(test)]
#[path = "zip_tests.rs"]
mod tests;
