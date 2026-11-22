//! Other CAD systems' native files that keep their exact geometry as Parasolid transmit files: Siemens NX parts
//! (`.prt`), Solid Edge parts (`.par`, `.psm`, `.pwd`) and JT files that carry precise geometry (XT B-Rep segments).
//!
//! - **Solid Edge**: an OLE compound file whose `PARASOLID` storage holds the model as `STREAM0.P_B` (more bodies as
//!   further `STREAMn.P_B`), each a binary transmit file, sometimes behind the `**ABCDEFGH…` text header.
//! - **NX**: an `SPLMSSTR` container. Its part stream holds zlib-packed transmit files: the model is the one titled
//!   `TRANSMIT FILE (partition)`; `(deltas)` files are rollback data and plain ones are features' tool bodies. Older
//!   parts keep only a plain one, behind a line of asterisks.
//! - **JT**: a table of segments. A segment of type 17 (XT B-Rep) or 30 (Multi XT B-Rep) holds one part's
//!   transmit file, zlib-packed (JT 9) or xz-packed (JT 10). Files saved for viewing only carry meshes instead.

const OLE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
const MAX_INFLATE: usize = 512 * 1024 * 1024;

/// What kind of native file the bytes are, when one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    Nx,
    SolidEdge,
    Jt,
}

pub fn kind(bytes: &[u8]) -> Option<Native> {
    if bytes.starts_with(b"SPLMSSTR") {
        Some(Native::Nx)
    } else if bytes.starts_with(b"Version ") && bytes.get(..80).is_some_and(|h| find(h, b" JT").is_some()) {
        Some(Native::Jt)
    } else if bytes.starts_with(OLE) && find(bytes, &utf16("PARASOLID")).is_some() {
        Some(Native::SolidEdge)
    } else {
        None
    }
}

/// The transmit files of a Solid Edge part: `PARASOLID/STREAMn.P_B`, in order.
pub fn solid_edge(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    use std::io::Read;
    let mut file = cfb::CompoundFile::open(std::io::Cursor::new(bytes)).map_err(|e| format!("not a Solid Edge file ({e})"))?;
    let mut names: Vec<String> = file
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().to_string_lossy().replace('\\', "/"))
        .filter(|n| {
            let up = n.to_ascii_uppercase();
            up.contains("/PARASOLID/STREAM") && up.ends_with(".P_B")
        })
        .collect();
    names.sort_by_key(|n| (n.len(), n.clone()));
    let mut out = Vec::new();
    for name in names {
        let mut data = Vec::new();
        if file.open_stream(&name).and_then(|mut s| s.read_to_end(&mut data)).is_err() {
            continue;
        }
        if let Some(at) = transmit_at(&data) {
            out.push(data[at..].to_vec());
        }
    }
    Ok(out)
}

/// The model of an NX part: its partition transmit file (or the one plain transmit file older parts keep).
pub fn nx(bytes: &[u8]) -> Vec<Vec<u8>> {
    let found = zlib_transmit_files(bytes);
    let title = |ps: &[u8]| {
        let head = &ps[..ps.len().min(96)];
        if find(head, b"(partition)").is_some() {
            0
        } else if find(head, b"(deltas)").is_some() {
            3
        } else {
            1
        }
    };
    let partitions: Vec<Vec<u8>> = found.iter().filter(|(_, ps)| title(ps) == 0).map(|(_, ps)| ps.clone()).collect();
    if !partitions.is_empty() {
        return partitions;
    }
    // No partition: the plain file the asterisk line introduces (feature tool bodies follow binary data instead).
    let starred =
        found.iter().filter(|(at, ps)| title(ps) == 1 && bytes[at.saturating_sub(8)..*at].iter().filter(|&&b| b == b'*').count() >= 4);
    let plain: Vec<Vec<u8>> = starred.map(|(_, ps)| ps.clone()).collect();
    if !plain.is_empty() {
        return plain;
    }
    found.into_iter().filter(|(_, ps)| title(ps) == 1).take(1).map(|(_, ps)| ps).collect()
}

/// What a JT file holds.
#[derive(Debug, Default)]
pub struct Jt {
    pub version: String,
    /// One transmit file per XT B-Rep segment, in the file's table order.
    pub parts: Vec<Vec<u8>>,
    /// Segments of mesh (shape level-of-detail) data.
    pub meshes: usize,
    /// JT B-Rep segments (the format's own exact geometry, not read here).
    pub jt_breps: usize,
}

/// The XT B-Rep parts of a JT file.
pub fn jt(bytes: &[u8]) -> Result<Jt, String> {
    let bad = |m: &str| format!("not a readable JT file ({m})");
    let version: String = bytes.get(..80).ok_or_else(|| bad("too short"))?.iter().map(|&b| char::from(b)).collect();
    let version = version.trim_end_matches(['\0', ' ', '\n', '\r']).trim().to_string();
    let major: f64 = version.split_whitespace().nth(1).and_then(|v| v.parse().ok()).ok_or_else(|| bad("no version"))?;
    if bytes[80] != 0 {
        return Err("big-endian JT files aren't supported".into());
    }
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let u64_at = |at: usize| bytes.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])) as usize);
    let wide = major >= 10.0;
    let toc = if wide { u64_at(85) } else { u32_at(85) }.ok_or_else(|| bad("no table of contents"))?;
    let count = u32_at(toc).ok_or_else(|| bad("no table of contents"))?;
    let entry = if wide { 32 } else { 28 };
    if count > 1_000_000 || toc + 4 + count * entry > bytes.len() {
        return Err(bad("table of contents out of range"));
    }
    let mut out = Jt { version, ..Jt::default() };
    for i in 0..count {
        let e = toc + 4 + i * entry + 16;
        let (offset, length, attrs) =
            if wide { (u64_at(e), u32_at(e + 8), u32_at(e + 12)) } else { (u32_at(e), u32_at(e + 4), u32_at(e + 8)) };
        let (Some(offset), Some(length), Some(attrs)) = (offset, length, attrs) else { continue };
        match attrs >> 24 {
            2 => out.jt_breps += 1,
            6..=16 => out.meshes += 1,
            17 | 30 => {
                let Some(seg) = bytes.get(offset..offset.saturating_add(length)) else { continue };
                let Some(data) = segment_data(seg) else { continue };
                // A segment can hold several (Multi XT B-Rep); each starts with its own header.
                let mut at = 0;
                while let Some(k) = data.get(at..).and_then(transmit_at) {
                    let start = at + k;
                    let next = data.get(start + 4..).and_then(transmit_at).map(|n| start + 4 + n).unwrap_or(data.len());
                    out.parts.push(data[start..next].to_vec());
                    at = next;
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// A JT segment's data after the 24-byte header, unpacked.
fn segment_data(seg: &[u8]) -> Option<Vec<u8>> {
    let i32_at = |at: usize| seg.get(at..at + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let flag = i32_at(24)?;
    if flag != 2 && flag != 3 {
        return Some(seg.get(24..)?.to_vec());
    }
    let len = usize::try_from(i32_at(28)?).ok()?;
    let algorithm = *seg.get(32)?;
    let packed = seg.get(33..(33 + len.saturating_sub(1)).min(seg.len()))?;
    match algorithm {
        1 => Some(packed.to_vec()),
        2 => miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, MAX_INFLATE).ok(),
        3 => {
            let mut out = Capped(Vec::new());
            lzma_rs::xz_decompress(&mut std::io::Cursor::new(packed), &mut out).ok()?;
            Some(out.0)
        }
        _ => None,
    }
}

/// A buffer that refuses to grow past `MAX_INFLATE` (a hostile stream can claim any size).
struct Capped(Vec<u8>);

impl std::io::Write for Capped {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + buf.len() > MAX_INFLATE {
            return Err(std::io::Error::other("segment too large"));
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Where a binary transmit file starts in `data` (its `PS\0\0` followed by the `TRANSMIT` title).
fn transmit_at(data: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(k) = data.get(from..).and_then(|d| find(d, b"PS\0\0")) {
        let at = from + k;
        if find(&data[at..(at + 96).min(data.len())], b"TRANSMIT").is_some() {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Every zlib stream in `bytes` that unpacks to a transmit file, with where it starts.
fn zlib_transmit_files(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 2 < bytes.len() {
        let (c, f) = (bytes[at], bytes[at + 1]);
        if c == 0x78 && (u16::from(c) << 8 | u16::from(f)) % 31 == 0 {
            let mut d = miniz_oxide::inflate::stream::InflateState::new_boxed(miniz_oxide::DataFormat::Zlib);
            let mut buf = vec![0u8; 96];
            let r = miniz_oxide::inflate::stream::inflate(&mut d, &bytes[at..], &mut buf, miniz_oxide::MZFlush::None);
            if r.bytes_written >= 4
                && buf.starts_with(b"PS\0\0")
                && let Ok(o) = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&bytes[at..], MAX_INFLATE)
            {
                out.push((at, o));
            }
        }
        at += 1;
    }
    out
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A Solid Edge-style compound file holding `ps` as `PARASOLID/STREAM0.P_B`: for tests.
#[doc(hidden)]
pub fn solid_edge_container(ps: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut file = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new()))?;
    file.create_storage("/PARASOLID")?;
    file.create_stream("/PARASOLID/STREAM0.P_B")?.write_all(ps)?;
    file.create_stream("/JSitesList")?.write_all(&[1, 0, 0, 0, 0, 0, 0, 0])?;
    file.flush()?;
    Ok(file.into_inner().into_inner())
}

/// An NX-style container: the header, other data, then `ps` zlib-packed behind a line of asterisks: for tests.
#[doc(hidden)]
pub fn nx_container(ps: &[u8]) -> Vec<u8> {
    let mut file = b"SPLMSSTR\x06\0\0\0/Root/UG_PART/UG_PART".to_vec();
    file.extend_from_slice(&[0x55; 300]);
    file.extend_from_slice(b"**************************\n");
    file.extend(miniz_oxide::deflate::compress_to_vec_zlib(ps, 6));
    file.extend_from_slice(&[0; 64]);
    file
}

/// A JT 9 file with one zlib-packed XT B-Rep segment (and one mesh segment) around `ps`: for tests.
#[doc(hidden)]
pub fn jt_container(ps: &[u8]) -> Vec<u8> {
    let mut file = format!("{:<80}", "Version 9.5 JT").into_bytes();
    file.push(0);
    file.extend_from_slice(&0i32.to_le_bytes());
    let toc_at = file.len();
    file.extend_from_slice(&0i32.to_le_bytes());
    file.extend_from_slice(&[0u8; 16]);
    // The XT segment: header, compression flag, packed length (with the algorithm byte), algorithm, data.
    let mut element = (ps.len() as u32).to_le_bytes().to_vec();
    element.extend_from_slice(ps);
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&element, 6);
    let mut seg = vec![7u8; 16];
    seg.extend_from_slice(&17i32.to_le_bytes());
    seg.extend_from_slice(&((24 + 9 + z.len()) as i32).to_le_bytes());
    seg.extend_from_slice(&2i32.to_le_bytes());
    seg.extend_from_slice(&((z.len() + 1) as i32).to_le_bytes());
    seg.push(2);
    seg.extend_from_slice(&z);
    let seg_at = file.len();
    file.extend_from_slice(&seg);
    let toc = file.len();
    file[toc_at..toc_at + 4].copy_from_slice(&(toc as i32).to_le_bytes());
    file.extend_from_slice(&2i32.to_le_bytes());
    for (at, len, ty) in [(seg_at, seg.len(), 17u32), (0, 0, 7)] {
        file.extend_from_slice(&[7u8; 16]);
        file.extend_from_slice(&(at as i32).to_le_bytes());
        file.extend_from_slice(&(len as i32).to_le_bytes());
        file.extend_from_slice(&(ty << 24).to_le_bytes());
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jt_segment_round_trip() {
        let ps = b"PS\0\0\0?: TRANSMIT FILE created by modeller version 3000298 and some body".to_vec();
        let file = jt_container(&ps);
        assert_eq!(kind(&file), Some(Native::Jt));
        let jt = jt(&file).expect("reads");
        assert_eq!(jt.version, "Version 9.5 JT");
        assert_eq!(jt.parts, vec![ps]);
        assert_eq!(jt.meshes, 1);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(b"SPLMSSTR\x06\0\0\0"), Some(Native::Nx));
        assert_eq!(kind(b"#UGC:2 PART 1234"), None);
        assert_eq!(kind(b"PS\0\0"), None);
    }
}
