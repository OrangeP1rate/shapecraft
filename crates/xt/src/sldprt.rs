//! SolidWorks part files (`.sldprt`): the Parasolid partition each configuration keeps.
//!
//! A part stores its solid model as a Parasolid partition transmit file (the neutral binary `PS\0\0` form this
//! crate already reads). Two containers are in use:
//! - **2015 and later**: a run of sections, each `14 00 06 00 08 00`, four u32s (two unknown, compressed size,
//!   uncompressed size), a u32 name length, the name with each byte's nibbles swapped, then raw-deflate data.
//! - **Before 2015**: an OLE compound file with the same names as streams (`Contents/Config-0-Partition`).
//!
//! A `Config-N-Partition` payload is either the transmit file itself or a 28-byte wrapper (u32, a 16-byte id,
//! uncompressed size, compressed size) around a zlib stream holding it.
//!
//! Newer versions (2021 on) may leave the partition empty when the part's bodies came from an imported file and
//! keep them in `Config-N-FeatureBodies/LocalBodies` instead: a header naming the source file (UTF-16), then the
//! same wrapper around a body transmit file, once per body. Older ones (2017–2020) keep such bodies packed inside
//! `Config-N-ResolvedFeatures`.

/// One Parasolid partition found in a part file.
#[derive(Debug, Clone)]
pub struct Partition {
    /// The section's name, as `Contents/Config-0-Partition`.
    pub name: String,
    /// The configuration number (`Config-N-…`), when the name carries one.
    pub config: Option<u32>,
    pub kind: PartitionKind,
    /// The transmit file (starts with `PS\0\0`).
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionKind {
    /// The configuration's model (`Config-N-Partition`).
    Model,
    /// Bodies a part was derived from (`Config-N-StockBodies`).
    Stock,
    /// Hidden working bodies (`Config-N-GhostPartition`).
    Ghost,
    /// Bodies of an imported-geometry feature (`Config-N-FeatureBodies/LocalBodies`), one per entry.
    Local,
    /// Bodies packed in a configuration's feature data (`Config-N-ResolvedFeatures`): older imported features.
    Resolved,
    Other,
}

const SECTION: &[u8] = b"\x14\x00\x06\x00\x08\x00";
const OLE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
/// Most bytes one section may inflate to (a partition of a large part reaches tens of MB).
const MAX_SECTION: usize = 512 * 1024 * 1024;

/// True when the bytes look like a SolidWorks document (part, assembly or drawing).
pub fn is_solidworks(bytes: &[u8]) -> bool {
    if bytes.starts_with(OLE) {
        return find(bytes, b"C\0o\0n\0t\0e\0n\0t\0s").is_some();
    }
    bytes.len() > 32 && find(&bytes[..bytes.len().min(4096)], SECTION).is_some() && sections(bytes).next().is_some()
}

/// Every Parasolid partition in a part, models first, in configuration order.
pub fn partitions(bytes: &[u8]) -> Vec<Partition> {
    let mut out: Vec<Partition> = Vec::new();
    let mut add = |name: String, payload: &[u8]| {
        let Some(kind) = kind_of(&name) else { return };
        // The section index repeats some names; keep the first copy of each.
        if out.iter().any(|p| p.name == name || p.name.starts_with(&format!("{name}#"))) {
            return;
        }
        if matches!(kind, PartitionKind::Local | PartitionKind::Resolved) {
            for (k, ps) in transmit_files(payload).into_iter().enumerate() {
                out.push(Partition { config: config_of(&name), name: format!("{name}#{k}"), kind, bytes: ps });
            }
        } else if let Some(ps) = unwrap(payload) {
            out.push(Partition { config: config_of(&name), name, kind, bytes: ps });
        }
    };
    if bytes.starts_with(OLE) {
        if let Ok(mut file) = cfb::CompoundFile::open(std::io::Cursor::new(bytes)) {
            let names: Vec<String> = file
                .walk()
                .filter(|e| e.is_stream())
                .map(|e| e.path().to_string_lossy().trim_start_matches(['/', '\\']).replace('\\', "/"))
                .collect();
            for name in names {
                use std::io::Read;
                let mut data = Vec::new();
                if file.open_stream(&name).and_then(|mut s| s.read_to_end(&mut data)).is_ok() {
                    add(name, &data);
                }
            }
        }
    } else {
        for (name, data) in sections(bytes) {
            add(name, &data);
        }
    }
    let rank = |k: PartitionKind| match k {
        PartitionKind::Model => 0,
        PartitionKind::Stock => 1,
        PartitionKind::Local | PartitionKind::Resolved => 1,
        PartitionKind::Ghost => 2,
        PartitionKind::Other => 3,
    };
    out.sort_by_key(|p| (rank(p.kind), p.config.unwrap_or(u32::MAX)));
    out
}

/// The model partition of the first configuration: what the part shows when opened.
pub fn model(bytes: &[u8]) -> Option<Partition> {
    partitions(bytes).into_iter().find(|p| p.kind == PartitionKind::Model)
}

fn kind_of(name: &str) -> Option<PartitionKind> {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    Some(if name.contains("-FeatureBodies/") && leaf == "LocalBodies" {
        PartitionKind::Local
    } else if leaf.ends_with("-ResolvedFeatures") {
        PartitionKind::Resolved
    } else if leaf.ends_with("-GhostPartition") {
        PartitionKind::Ghost
    } else if leaf.ends_with("-Partition") {
        PartitionKind::Model
    } else if leaf.ends_with("-StockBodies") {
        PartitionKind::Stock
    } else if leaf.contains("Partition") {
        PartitionKind::Other
    } else {
        return None;
    })
}

fn config_of(name: &str) -> Option<u32> {
    let rest = &name[name.find("Config-")? + "Config-".len()..];
    rest.split(['-', '/']).next()?.parse().ok()
}

/// Every transmit file zlib-packed anywhere in a payload, in order.
fn transmit_files(payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 2 < payload.len() {
        let (c, f) = (payload[at], payload[at + 1]);
        // A zlib header: deflate, 32K window, check bits.
        if c == 0x78 && (u16::from(c) << 8 | u16::from(f)) % 31 == 0 {
            let mut d = miniz_oxide::inflate::stream::InflateState::new_boxed(miniz_oxide::DataFormat::Zlib);
            let mut buf = vec![0u8; 96];
            let r = miniz_oxide::inflate::stream::inflate(&mut d, &payload[at..], &mut buf, miniz_oxide::MZFlush::None);
            if r.bytes_written >= 4
                && buf.starts_with(b"PS\0\0")
                && let Ok(o) = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&payload[at..], MAX_SECTION)
            {
                // The search goes on inside this stream's data: a false header there fails the 96-byte peek.
                out.push(o);
            }
        }
        at += 1;
    }
    out
}

/// The transmit file inside a partition payload: as is, behind the 28-byte wrapper, or zlib at the start.
fn unwrap(payload: &[u8]) -> Option<Vec<u8>> {
    let is_ps = |b: &[u8]| b.starts_with(b"PS\0\0") && find(&b[..b.len().min(96)], b"TRANSMIT").is_some();
    if is_ps(payload) {
        return Some(payload.to_vec());
    }
    for at in [28usize, 0] {
        if let Some(z) = payload.get(at..)
            && z.first() == Some(&0x78)
            && let Ok(o) = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(z, MAX_SECTION)
            && is_ps(&o)
        {
            return Some(o);
        }
    }
    None
}

/// The named sections of a 2015-or-later container, inflated. Sections that don't inflate are skipped (the
/// trailing index repeats the names without data).
fn sections(bytes: &[u8]) -> impl Iterator<Item = (String, Vec<u8>)> + '_ {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        loop {
            let i = at + find(bytes.get(at..)?, SECTION)?;
            at = i + 1;
            let u = |k: usize| bytes.get(i + 6 + 4 * k..i + 10 + 4 * k).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
            let (Some(csize), Some(nlen)) = (u(2), u(4)) else { return None };
            if nlen == 0 || nlen > 512 {
                continue;
            }
            let name_at = i + 26;
            let Some(raw) = bytes.get(name_at..name_at + nlen) else { continue };
            let name: String = raw.iter().map(|&b| char::from(b.rotate_left(4))).collect();
            if !name.chars().all(|c| (' '..='~').contains(&c)) {
                continue;
            }
            let data_at = name_at + nlen;
            let Some(data) = bytes.get(data_at..data_at.saturating_add(csize)) else { continue };
            let Ok(out) = miniz_oxide::inflate::decompress_to_vec_with_limit(data, MAX_SECTION) else { continue };
            if out.is_empty() {
                continue;
            }
            at = data_at + csize;
            return Some((name, out));
        }
    })
}

/// A 2015-style container holding each (name, transmit file) as a wrapped partition section: for tests, and to
/// check the reader against files we make ourselves.
#[doc(hidden)]
pub fn container(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut file = b"\0\0\0\0".to_vec();
    for (name, ps) in parts {
        let z = miniz_oxide::deflate::compress_to_vec_zlib(ps, 6);
        let mut payload = vec![0u8; 20];
        payload.extend_from_slice(&(ps.len() as u32).to_le_bytes());
        payload.extend_from_slice(&(z.len() as u32).to_le_bytes());
        payload.extend_from_slice(&z);
        let data = miniz_oxide::deflate::compress_to_vec(&payload, 6);
        file.extend_from_slice(SECTION);
        for v in [0u32, 0, data.len() as u32, payload.len() as u32, name.len() as u32] {
            file.extend_from_slice(&v.to_le_bytes());
        }
        file.extend(name.bytes().map(|b| b.rotate_left(4)));
        file.extend_from_slice(&data);
    }
    file
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_configs() {
        assert_eq!(kind_of("Contents/Config-0-Partition"), Some(PartitionKind::Model));
        assert_eq!(kind_of("Contents/Config-0-GhostPartition"), Some(PartitionKind::Ghost));
        assert_eq!(kind_of("Contents/Config-2-StockBodies"), Some(PartitionKind::Stock));
        assert_eq!(kind_of("Contents/Config-0-ResolvedFeatures"), Some(PartitionKind::Resolved));
        assert_eq!(kind_of("Contents/Config-0-ModelHeader"), None);
        assert_eq!(config_of("Contents/Config-12-Partition"), Some(12));
        assert_eq!(config_of("Header2"), None);
        assert_eq!(config_of("Config-3-FeatureBodies/LocalBodies"), Some(3));
        assert_eq!(kind_of("Config-0-FeatureBodies/LocalBodies"), Some(PartitionKind::Local));
    }

    /// A section built the way SolidWorks writes them round-trips through `sections` and `unwrap`.
    #[test]
    fn synthetic_section() {
        let ps = b"PS\0\0\0?: TRANSMIT FILE (partition) created by modeller version 3000298".to_vec();
        let name = "Contents/Config-0-Partition";
        let mut file = b"junk".to_vec();
        file.extend(container(&[("Contents/Config-0-GhostPartition", &ps), (name, &ps)]));
        assert!(is_solidworks(&file));
        let m = model(&file).expect("model partition");
        assert_eq!(m.name, name);
        assert_eq!(m.config, Some(0));
        assert_eq!(m.bytes, ps);
        assert_eq!(partitions(&file).len(), 2);
    }
}
