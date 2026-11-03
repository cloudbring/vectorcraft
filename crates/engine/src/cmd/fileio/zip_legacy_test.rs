use super::DEFLATE_CHUNK;
use flate2::Crc;
use std::io::Write as _;

const DOS_DATE: u16 = (1 << 5) | 1;
const UTF8_NAMES: u16 = 1 << 11;
const VERSION: u16 = 20;

/// The archive of `files` (`(name, bytes)`; `/` separates folders in a name), stored without
/// compression. Fails past the limits of a ZIP without its 64-bit extension (65534 files, 4 GB; 65535 is the ZIP64 marker).
pub fn store<N: AsRef<str>, B: AsRef<[u8]>>(files: &[(N, B)]) -> Result<Vec<u8>, String> {
    let too_big = || "too much to zip: at most 65534 files and 4 GB".to_string();
    let count = u16::try_from(files.len()).ok().filter(|&c| c != u16::MAX).ok_or_else(too_big)?;
    let mut out = Vec::with_capacity(files.iter().map(|(n, b)| 76 + 2 * n.as_ref().len() + b.as_ref().len()).sum::<usize>() + 22);
    let mut central = Vec::new();
    for (name, bytes) in files {
        let (name, bytes) = (name.as_ref().as_bytes(), bytes.as_ref());
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        let size = u32::try_from(bytes.len()).map_err(|_| too_big())?;
        let name_len = u16::try_from(name.len()).map_err(|_| format!("file name too long to zip: {}", String::from_utf8_lossy(name)))?;
        let mut crc = Crc::new();
        crc.update(bytes);
        // The fields both headers share: flags, method (0: stored), time, date, CRC, sizes, name length.
        let mut shared = Vec::with_capacity(26);
        for v in [UTF8_NAMES, 0, 0, DOS_DATE] {
            shared.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc.sum(), size, size] {
            shared.extend_from_slice(&v.to_le_bytes());
        }
        shared.extend_from_slice(&name_len.to_le_bytes());
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&shared);
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(name);
        out.extend_from_slice(bytes);
        // Central directory record.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&VERSION.to_le_bytes()); // made by
        central.extend_from_slice(&VERSION.to_le_bytes()); // needed to extract
        central.extend_from_slice(&shared);
        // Extra and comment lengths, disk number, internal and external attributes.
        central.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let start = u32::try_from(out.len()).map_err(|_| too_big())?;
    let central_len = u32::try_from(central.len()).map_err(|_| too_big())?;
    out.extend_from_slice(&central);
    // End of central directory.
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // this disk, the directory's disk
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&central_len.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    Ok(out)
}

/// The archive of `files` (`(name, bytes)`; `/` separates folders in a name), each deflated, or
/// stored when deflating doesn't make it smaller, every entry dated `(time, date)` (MS-DOS format).
/// Fails past the same limits as [`store`]. `stop` is asked before each [`DEFLATE_CHUNK`] bytes are
/// deflated; when it answers true, the archive is dropped and an error returned.
pub fn deflate<N: AsRef<str>, B: AsRef<[u8]>>(files: &[(N, B)], (time, date): (u16, u16), stop: impl Fn() -> bool) -> Result<Vec<u8>, String> {
    let too_big = || "too much to zip: at most 65534 files and 4 GB".to_string();
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, bytes) in files {
        let (name, bytes) = (name.as_ref(), bytes.as_ref());
        let mut crc = Crc::new();
        crc.update(bytes);
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        for chunk in bytes.chunks(DEFLATE_CHUNK) {
            if stop() {
                return Err("stopped before the archive was finished".into());
            }
            enc.write_all(chunk).map_err(|e| e.to_string())?;
        }
        let deflated = enc.finish().map_err(|e| e.to_string())?;
        let (method, data): (u16, &[u8]) = if deflated.len() < bytes.len() { (8, &deflated) } else { (0, bytes) };
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        let (packed, size) = (u32::try_from(data.len()).map_err(|_| too_big())?, u32::try_from(bytes.len()).map_err(|_| too_big())?);
        let name_len = u16::try_from(name.len()).map_err(|_| format!("file name too long to zip: {name}"))?;
        // Version 2.0, UTF-8 names, method, time, date, CRC-32, sizes, name length, no extra field.
        let common =
            [&VERSION.to_le_bytes()[..], &UTF8_NAMES.to_le_bytes(), &method.to_le_bytes(), &time.to_le_bytes(), &date.to_le_bytes()].concat();
        let sums = [crc.sum().to_le_bytes(), packed.to_le_bytes(), size.to_le_bytes()].concat();
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&common);
        out.extend_from_slice(&sums);
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&VERSION.to_le_bytes());
        central.extend_from_slice(&common);
        central.extend_from_slice(&sums);
        central.extend_from_slice(&name_len.to_le_bytes());
        // No extra field or comment, disk 0, no attributes.
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let count = u16::try_from(files.len()).ok().filter(|&c| c != u16::MAX).ok_or_else(too_big)?;
    let (start, len) = (u32::try_from(out.len()).map_err(|_| too_big())?, u32::try_from(central.len()).map_err(|_| too_big())?);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}
