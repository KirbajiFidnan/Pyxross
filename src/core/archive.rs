//! Pure STORED-ZIP archive codec (D80 single-file `.pyxross` container).
//!
//! Packs/unpacks an in-memory archive (no filesystem access) into a **valid,
//! deterministic ZIP file** — one any `unzip`/7z can open. Entries are stored
//! without compression (method 0), with no timestamps, no extra fields, no
//! comments, so identical input always produces identical bytes.
//!
//! The packed layout matches the directory `.pyxross` tree exactly:
//! `manifest.json`, `layers/<id>.png`, `tiles/<id>.png`.
//!
//! No external dependencies: CRC-32 uses a local const lookup table. ZIP64 is
//! deliberately unsupported (archives are limited to 4 GiB / 65535 entries).

use std::fmt;

/// Errors produced by the archive codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// The input does not start with a ZIP signature (`PK\x03\x04` /
    /// `PK\x05\x06`).
    BadMagic,
    /// The archive is structurally broken: missing/incomplete EOCD, central
    /// directory or local headers out of bounds, header cut mid-read, or
    /// local-header crc/size/name disagreeing with the central directory.
    Truncated,
    /// A stored payload's CRC-32 does not match the declared checksum.
    CrcMismatch(String),
    /// Declared payload exceeds the ZIP32 limit (more than 4 GiB across all
    /// entries, or an entry claiming more data than the input holds).
    TooLarge,
    /// More than 65535 entries (ZIP32 EOCD limit; Zip64 unsupported).
    TooManyEntries,
    /// Entry name violates the safety rules (empty, >255 bytes, NUL, leading
    /// `/` or `\`, or a `.`/`..` segment).
    InvalidName,
    /// Compression method other than STORED (0) — only stored entries are
    /// supported.
    UnsupportedMethod(u16),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::BadMagic => write!(f, "not a ZIP archive (bad magic)"),
            ArchiveError::Truncated => write!(f, "truncated or inconsistent ZIP archive"),
            ArchiveError::CrcMismatch(name) => write!(f, "CRC-32 mismatch for entry {name:?}"),
            ArchiveError::TooLarge => write!(f, "archive exceeds the ZIP32 4 GiB limit"),
            ArchiveError::TooManyEntries => {
                write!(f, "more than 65535 entries (ZIP32 limit, no Zip64)")
            }
            ArchiveError::InvalidName => write!(f, "unsafe entry name"),
            ArchiveError::UnsupportedMethod(m) => {
                write!(f, "unsupported compression method {m} (only STORED)")
            }
        }
    }
}

impl std::error::Error for ArchiveError {}

// ---------------------------------------------------------------------------
// CRC-32 (IEEE 802.3), table-based, deterministic.
// ---------------------------------------------------------------------------

const CRC32_TABLE: [u32; 256] = build_crc32_table();

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// CRC-32 (IEEE 802.3, reflected, init 0xFFFFFFFF, final xor 0xFFFFFFFF) —
/// the checksum ZIP uses.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// Name safety (enforced on both pack and unpack).
// ---------------------------------------------------------------------------

const MAX_NAME_LEN: usize = 255;

fn validate_name(name: &str) -> Result<(), ArchiveError> {
    if name.is_empty() {
        return Err(ArchiveError::InvalidName);
    }
    if name.len() > MAX_NAME_LEN {
        return Err(ArchiveError::InvalidName);
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err(ArchiveError::InvalidName);
    }
    // A backslash anywhere is a Windows path separator; banning it keeps
    // names portable and blocks `a\..\b`-style smuggling on extraction.
    if name.contains('\\') {
        return Err(ArchiveError::InvalidName);
    }
    if name.contains('\0') {
        return Err(ArchiveError::InvalidName);
    }
    // Both separators count: `/` on Unix, `\` on Windows. A `.`/`..` segment
    // anywhere is a traversal hazard.
    for segment in name.split(['/', '\\']) {
        if segment == "." || segment == ".." {
            return Err(ArchiveError::InvalidName);
        }
    }
    Ok(())
}

/// ZIP32 size guard: the whole payload must fit u32 offset/size fields.
fn check_total(total: u64) -> Result<(), ArchiveError> {
    if total > u32::MAX as u64 {
        return Err(ArchiveError::TooLarge);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pack
// ---------------------------------------------------------------------------

/// Packs entries into a valid, deterministic STORED ZIP file.
///
/// Entry order is preserved. `name` is a slash-separated relative path (e.g.
/// `layers/1.png`); entries must satisfy the name-safety rules.
pub fn pack(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, ArchiveError> {
    if entries.len() > 0xFFFF {
        return Err(ArchiveError::TooManyEntries);
    }
    for (name, _) in entries {
        validate_name(name)?;
    }
    check_total(entries.iter().map(|(_, d)| d.len() as u64).sum())?;

    // Full layout size (LFH+name+data per entry, CD entries, EOCD) — must fit
    // u32 so every recorded offset stays valid.
    let mut layout: u64 = 22; // EOCD
    for (name, data) in entries {
        layout += 30 + name.len() as u64 + data.len() as u64;
        layout += 46 + name.len() as u64;
    }
    check_total(layout)?;

    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<u64> = Vec::with_capacity(entries.len());
    let mut crcs: Vec<u32> = Vec::with_capacity(entries.len());

    for (name, data) in entries {
        offsets.push(out.len() as u64);
        let crc = crc32(data);
        crcs.push(crc);
        // Local file header (30 bytes).
        push_u32(&mut out, 0x0403_4b50); // sig
        push_u16(&mut out, 20); // version needed
        push_u16(&mut out, 0); // flags
        push_u16(&mut out, 0); // method: stored
        push_u16(&mut out, 0); // mod time (deterministic)
        push_u16(&mut out, 0); // mod date (deterministic)
        push_u32(&mut out, crc);
        push_u32(&mut out, data.len() as u32); // compressed size == stored
        push_u32(&mut out, data.len() as u32); // uncompressed size
        push_u16(&mut out, name.len() as u16); // name length (<=255 by safety)
        push_u16(&mut out, 0); // extra length
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
    }

    let cd_offset = out.len() as u64;
    for (i, (name, data)) in entries.iter().enumerate() {
        // Central directory header (46 bytes).
        push_u32(&mut out, 0x0201_4b50); // sig
        push_u16(&mut out, 20); // version made by
        push_u16(&mut out, 20); // version needed
        push_u16(&mut out, 0); // flags
        push_u16(&mut out, 0); // method: stored
        push_u16(&mut out, 0); // mod time
        push_u16(&mut out, 0); // mod date
        push_u32(&mut out, crcs[i]);
        push_u32(&mut out, data.len() as u32);
        push_u32(&mut out, data.len() as u32);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0); // extra length
        push_u16(&mut out, 0); // comment length
        push_u16(&mut out, 0); // disk number start
        push_u16(&mut out, 0); // internal attrs
        push_u32(&mut out, 0); // external attrs
        push_u32(&mut out, offsets[i] as u32); // local header offset
        out.extend_from_slice(name.as_bytes());
    }
    let cd_size = (out.len() as u64 - cd_offset) as u32;

    // End of central directory (22 bytes).
    push_u32(&mut out, 0x0605_4b50); // sig
    push_u16(&mut out, 0); // disk number
    push_u16(&mut out, 0); // disk with CD
    push_u16(&mut out, entries.len() as u16); // entries on this disk
    push_u16(&mut out, entries.len() as u16); // total entries
    push_u32(&mut out, cd_size);
    push_u32(&mut out, cd_offset as u32);
    push_u16(&mut out, 0); // comment length

    Ok(out)
}

// ---------------------------------------------------------------------------
// Unpack
// ---------------------------------------------------------------------------

/// Locates the End-Of-Central-Directory record. The EOCD must end exactly at
/// `bytes.len()` (comment length included), which rules out false signature
/// matches inside payload data. Returns the EOCD offset.
fn find_eocd(bytes: &[u8]) -> Result<usize, ArchiveError> {
    if bytes.len() < 22 {
        return Err(ArchiveError::Truncated);
    }
    let mut p = bytes.len() - 22; // earliest a 22-byte EOCD can sit while ending at len
    loop {
        if bytes[p..p + 4] == [0x50, 0x4B, 0x05, 0x06] {
            let comment_len =
                u16::from_le_bytes([bytes[p + 20], bytes[p + 21]]) as usize;
            if p + 22 + comment_len == bytes.len() {
                return Ok(p);
            }
            // Signature inside data/comment — keep scanning backwards.
        }
        if p == 0 {
            break;
        }
        p -= 1;
    }
    Err(ArchiveError::Truncated)
}

fn read_u16(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([b[p], b[p + 1]])
}

fn read_u32(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
}

/// Unpacks a STORED ZIP archive produced by [`pack`] (or any stored zip), in
/// central-directory order.
///
/// Validation is strict: bad magic, structural inconsistencies and unsafe
/// names are all errors (never skipped entries); CRC-32 is verified per entry.
pub fn unpack(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ArchiveError> {
    // Accept both a local-file-header start (non-empty archive) and an
    // EOCD-only start (empty archive from pack(&[])).
    let looks_like_zip = bytes.len() >= 4
        && (bytes[0..4] == [0x50, 0x4B, 0x03, 0x04]
            || bytes[0..4] == [0x50, 0x4B, 0x05, 0x06]);
    if !looks_like_zip {
        return Err(ArchiveError::BadMagic);
    }

    let eocd = find_eocd(bytes)?;
    let total_entries = read_u16(bytes, eocd + 10) as usize; // total entries
    let cd_size = read_u32(bytes, eocd + 12) as usize;
    let cd_offset = read_u32(bytes, eocd + 16) as usize;
    if cd_offset as u64 + cd_size as u64 > bytes.len() as u64 {
        return Err(ArchiveError::Truncated);
    }
    if total_entries > 0xFFFF {
        return Err(ArchiveError::TooManyEntries); // defensive; u16 field caps this
    }

    // Walk the central directory.
    struct CdEntry {
        name: String,
        crc: u32,
        csize: u32,
        usize_: u32,
        lho: usize,
    }
    let mut list: Vec<CdEntry> = Vec::with_capacity(total_entries);
    let mut p = cd_offset;
    let cd_end = cd_offset + cd_size;
    for _ in 0..total_entries {
        if p + 46 > cd_end {
            return Err(ArchiveError::Truncated);
        }
        let sig = read_u32(bytes, p);
        if sig != 0x0201_4b50 {
            return Err(ArchiveError::Truncated);
        }
        let method = read_u16(bytes, p + 10);
        if method != 0 {
            return Err(ArchiveError::UnsupportedMethod(method));
        }
        let crc = read_u32(bytes, p + 16);
        let csize = read_u32(bytes, p + 20);
        let usize_ = read_u32(bytes, p + 24);
        let name_len = read_u16(bytes, p + 28) as usize;
        let extra_len = read_u16(bytes, p + 30) as usize;
        let comment_len = read_u16(bytes, p + 32) as usize;
        let lho = read_u32(bytes, p + 42) as usize;
        if p + 46 + name_len + extra_len + comment_len > cd_end {
            return Err(ArchiveError::Truncated);
        }
        let name_bytes = &bytes[p + 46..p + 46 + name_len];
        let name = std::str::from_utf8(name_bytes).map_err(|_| ArchiveError::InvalidName)?;
        validate_name(name)?;
        list.push(CdEntry {
            name: name.to_owned(),
            crc,
            csize,
            usize_,
            lho,
        });
        p += 46 + name_len + extra_len + comment_len;
    }

    // Declared payload across all entries must fit ZIP32.
    check_total(list.iter().map(|e| e.usize_ as u64).sum())?;

    // Extract, verifying each local header and checksum.
    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(list.len());
    for e in list {
        if e.csize != e.usize_ {
            return Err(ArchiveError::Truncated); // method 0 must store equal sizes
        }
        if e.lho as u64 + 30 > bytes.len() as u64 {
            return Err(ArchiveError::Truncated);
        }
        let lh = &bytes[e.lho..e.lho + 30];
        if read_u32(lh, 0) != 0x0403_4b50 {
            return Err(ArchiveError::Truncated);
        }
        if read_u16(lh, 8) != 0 {
            let m = read_u16(lh, 8);
            return Err(ArchiveError::UnsupportedMethod(m));
        }
        if read_u32(lh, 14) != e.crc || read_u32(lh, 18) != e.csize || read_u32(lh, 22) != e.usize_
        {
            return Err(ArchiveError::Truncated); // local vs central inconsistency
        }
        let name_len = read_u16(lh, 26) as usize;
        let extra_len = read_u16(lh, 28) as usize;
        if name_len != e.name.len()
            || &bytes[e.lho + 30..e.lho + 30 + name_len] != e.name.as_bytes()
        {
            return Err(ArchiveError::Truncated);
        }
        let data_start = e.lho as u64 + 30 + name_len as u64 + extra_len as u64;
        let data_end = data_start + e.csize as u64;
        if data_end > bytes.len() as u64 {
            // Entry claims more data than the input holds (inflated
            // declaration / declared too large).
            return Err(ArchiveError::TooLarge);
        }
        let data = &bytes[data_start as usize..data_end as usize];
        if crc32(data) != e.crc {
            return Err(ArchiveError::CrcMismatch(e.name));
        }
        out.push((e.name, data.to_vec()));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Little-endian writers.
// ---------------------------------------------------------------------------

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Builds a raw single-entry STORED ZIP byte string with a given name and
    /// data, bypassing `pack`'s name validation (for unpack-side tests).
    fn raw_zip(name: &[u8], data: &[u8]) -> Vec<u8> {
        let crc = crc32(data);
        let mut out = Vec::new();
        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, crc);
        push_u32(&mut out, data.len() as u32);
        push_u32(&mut out, data.len() as u32);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        let cd_offset = out.len() as u32;
        push_u32(&mut out, 0x0201_4b50);
        push_u16(&mut out, 20);
        push_u16(&mut out, 20);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, crc);
        push_u32(&mut out, data.len() as u32);
        push_u32(&mut out, data.len() as u32);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, 0); // external attrs
        push_u32(&mut out, 0); // local header offset (always 0 in raw_zip)
        out.extend_from_slice(name);
        let cd_size = out.len() as u32 - cd_offset;
        push_u32(&mut out, 0x0605_4b50);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 1);
        push_u16(&mut out, 1);
        push_u32(&mut out, cd_size);
        push_u32(&mut out, cd_offset);
        push_u16(&mut out, 0);
        out
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn roundtrip_preserves_order_and_bytes() {
        let entries: Vec<(&str, &[u8])> = vec![
            ("manifest.json", br#"{"format_version":8}"#),
            ("layers/1.png", b"\x89PNG-fake-layer-1"),
            ("tiles/2.png", b""),
            ("layers/3.png", b"bytes with \x00 binary \xFF\xFE content"),
        ];
        let zip = pack(&entries).unwrap();
        let got = unpack(&zip).unwrap();
        assert_eq!(got.len(), entries.len());
        for ((name, data), (g_name, g_data)) in entries.iter().zip(got.iter()) {
            assert_eq!(name, g_name);
            assert_eq!(*data, g_data.as_slice());
        }
    }

    #[test]
    fn output_is_deterministic() {
        let entries: Vec<(&str, &[u8])> = vec![("a", b"x"), ("layers/1.png", b"y")];
        assert_eq!(pack(&entries).unwrap(), pack(&entries).unwrap());
    }

    #[test]
    fn empty_archive_packs_and_unpacks() {
        let zip = pack(&[]).unwrap();
        assert!(!zip.is_empty());
        let got = unpack(&zip).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn bad_magic_is_rejected() {
        assert_eq!(unpack(b""), Err(ArchiveError::BadMagic));
        assert_eq!(unpack(b"hello world"), Err(ArchiveError::BadMagic));
        assert_eq!(unpack(&[0x00; 64]), Err(ArchiveError::BadMagic));
    }

    #[test]
    fn every_prefix_of_a_packed_archive_is_an_error() {
        let entries: Vec<(&str, &[u8])> = vec![
            ("manifest.json", b"abcd"),
            ("layers/1.png", b"efghijkl"),
            ("tiles/2.png", b"mnopqrstuv"),
        ];
        let zip = pack(&entries).unwrap();
        for cut in 0..zip.len() {
            let res = unpack(&zip[..cut]);
            assert!(res.is_err(), "prefix of length {cut} must fail");
        }
        // Full slice is fine.
        assert!(unpack(&zip).is_ok());
    }

    #[test]
    fn corrupted_payload_reports_crc_mismatch() {
        // Single entry "a" with data "hello": LFH (30) + name (1) => data at 31.
        let mut zip = pack(&[("a", b"hello")]).unwrap();
        zip[31] ^= 0x01;
        assert_eq!(unpack(&zip), Err(ArchiveError::CrcMismatch("a".to_owned())));
    }

    #[test]
    fn pack_rejects_unsafe_names() {
        let bad: &[&str] = &[
            "",
            "..",
            ".",
            "../x",
            "a/../b",
            "a/./b",
            "/absolute",
            "\\absolute",
            "a\\b",
            "a\0b",
            &"n".repeat(256),
        ];
        for name in bad {
            let res = pack(&[(name, b"")]);
            assert_eq!(res, Err(ArchiveError::InvalidName), "name {name:?}");
        }
    }

    #[test]
    fn unpack_rejects_unsafe_names() {
        let bad: &[&[u8]] = &[
            b"",
            b"..",
            b".",
            b"../x",
            b"a/../b",
            b"a/./b",
            b"/absolute",
            b"\\absolute",
            b"a\\b",
            b"a\0b",
            &b"n".repeat(256),
            &[0xFF, 0xFE], // invalid UTF-8
        ];
        for name in bad {
            let zip = raw_zip(name, b"");
            assert_eq!(unpack(&zip), Err(ArchiveError::InvalidName), "name {name:?}");
        }
        // A safe name still round-trips through the same raw layout.
        let zip = raw_zip(b"layers/1.png", b"data");
        assert_eq!(
            unpack(&zip).unwrap(),
            vec![("layers/1.png".to_owned(), b"data".to_vec())]
        );
    }

    #[test]
    fn too_many_entries_rejected_on_pack() {
        let names: Vec<String> = (0..=0xFFFF).map(|i| format!("e{i}")).collect();
        let empty: &[u8] = b"";
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), empty)).collect();
        assert_eq!(pack(&entries), Err(ArchiveError::TooManyEntries));
    }

    #[test]
    fn oversized_declaration_rejected() {
        // Total exactly at the ZIP32 boundary is fine.
        check_total(u32::MAX as u64).unwrap();
        // One byte past the boundary must fail.
        assert_eq!(check_total(u32::MAX as u64 + 1), Err(ArchiveError::TooLarge));

        // Unpack side: an entry claiming 1 GiB while the file holds ~100 bytes
        // is an inflated declaration. Inflate LFH *and* CD sizes consistently
        // so the local-vs-central check passes and the data-bounds guard fires.
        let mut zip = raw_zip(b"a", b"tiny");
        zip[18..22].copy_from_slice(&0x4000_0000u32.to_le_bytes()); // LFH csize
        zip[22..26].copy_from_slice(&0x4000_0000u32.to_le_bytes()); // LFH usize
        // CD begins at LFH(30) + name(1) + data(4) = 35; csize at +20, usize at +24.
        zip[35 + 20..35 + 24].copy_from_slice(&0x4000_0000u32.to_le_bytes());
        zip[35 + 24..35 + 28].copy_from_slice(&0x4000_0000u32.to_le_bytes());
        assert_eq!(unpack(&zip), Err(ArchiveError::TooLarge));
    }

    #[test]
    fn unsupported_method_rejected() {
        // Deflate (method 8) in the central directory must be refused.
        let mut zip = raw_zip(b"a", b"data");
        // Central directory starts right after the local entry:
        // LFH 30 + name 1 + data 4 = 35.
        zip[35 + 10..35 + 12].copy_from_slice(&8u16.to_le_bytes());
        assert_eq!(unpack(&zip), Err(ArchiveError::UnsupportedMethod(8)));
    }

    #[test]
    fn output_is_a_valid_zip_for_external_unzip() {
        // Integration check against a real unzip binary when available
        // (skipped silently if `unzip` is not installed).
        if std::process::Command::new("unzip")
            .arg("-v")
            .output()
            .is_err()
        {
            return;
        }
        let entries: Vec<(&str, &[u8])> = vec![
            ("manifest.json", br#"{"format_version":8}"#),
            ("layers/1.png", b"PNG-DATA-1"),
            ("tiles/2.png", b"PNG-DATA-2"),
        ];
        let zip = pack(&entries).unwrap();

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pyxross_archive_test_{}_{}.zip",
            std::process::id(),
            n
        ));
        std::fs::write(&path, &zip).unwrap();
        let status = std::process::Command::new("unzip")
            .arg("-t") // test integrity + CRC
            .arg(&path)
            .status()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(status.success(), "external `unzip -t` must accept the archive");
    }
}