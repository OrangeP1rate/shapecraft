//! History nodes as the releases of 2024 keep them: JSON, with each parameter value a little-endian binary literal in
//! base64. They are handed on in the shape the later MessagePack nodes have, so the rest of the reader sees one layout.
use rmpv::Value;

/// A history node kept as JSON: the root and lists as `[[ids]]`, a step `{version, functionName, stepName,
/// parameters}` as `[version, name, operation, [ids]]`, a parameter `{literalValue}` as `[[value]]`.
pub(crate) fn node(ty: i64, props: &[u8]) -> Option<Value> {
    let j: serde_json::Value = serde_json::from_slice(props).ok()?;
    let ids = |v: &serde_json::Value| {
        Value::Array(v.as_array().into_iter().flatten().filter_map(serde_json::Value::as_i64).take(1_000_000).map(Value::from).collect())
    };
    Some(match ty {
        2 => Value::Array(vec![
            Value::from(j["version"].as_i64().unwrap_or(0)),
            Value::from(j["stepName"].as_str().unwrap_or("")),
            Value::from(j["functionName"].as_str().unwrap_or("")),
            ids(&j["parameters"]),
        ]),
        0 | 1 => Value::Array(vec![ids(&j["children"])]),
        _ => {
            let bytes = base64(j["literalValue"].as_str()?)?;
            let mut at = 0;
            let v = literal(&bytes, &mut at, 0)?;
            Value::Array(vec![Value::Array(vec![v])])
        }
    })
}

/// One tagged value. The tag is a `u32`; what follows depends on it. The values come out tagged as the MessagePack
/// nodes tag them: a list `[9, [[v], …]]`, a map `[4, [key, [v], …]]`, a name `[7, id]`, a sketch `[10, id]`, a real
/// `[12, [[1, 1], x]]`, an integer `[2, n]`, a truth value `[1, b]`, a triple `[5, [x, y, z]]`, nothing `[0]`.
fn literal(b: &[u8], at: &mut usize, depth: usize) -> Option<Value> {
    if depth > 32 {
        return None;
    }
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = b.get(*at..at.checked_add(n)?)?;
        *at += n;
        Some(s)
    };
    let tag = u32::from_le_bytes(take(4)?.try_into().ok()?);
    let tagged = |tag: i64, v: Value| Value::Array(vec![Value::from(tag), v]);
    Some(match tag {
        0 | 10 => {
            let n = u64::from_le_bytes(take(8)?.try_into().ok()?);
            // Every element takes at least four bytes, so a count past the data is a broken file.
            if n > (b.len() / 4) as u64 {
                return None;
            }
            let mut items = Vec::new();
            for _ in 0..n {
                if tag == 10 {
                    let len = usize::try_from(u64::from_le_bytes(b.get(*at..at.checked_add(8)?)?.try_into().ok()?)).ok()?;
                    *at += 8;
                    let key = b.get(*at..at.checked_add(len)?)?;
                    *at += len;
                    items.push(Value::from(String::from_utf8_lossy(key).into_owned()));
                }
                items.push(Value::Array(vec![literal(b, at, depth + 1)?]));
            }
            tagged(if tag == 0 { 9 } else { 4 }, Value::Array(items))
        }
        1 => tagged(1, Value::from(*take(1)?.first()? != 0)),
        3 => {
            let x = f64::from_le_bytes(take(8)?.try_into().ok()?);
            tagged(12, Value::Array(vec![Value::Array(vec![Value::from(1), Value::from(1)]), Value::from(x)]))
        }
        4 => tagged(2, Value::from(i32::from_le_bytes(take(4)?.try_into().ok()?))),
        5 => tagged(7, Value::from(u64::from_le_bytes(take(8)?.try_into().ok()?))),
        6 => Value::Array(vec![Value::from(0)]),
        7 => tagged(10, Value::from(u64::from_le_bytes(take(8)?.try_into().ok()?))),
        8 => {
            let mut xyz = Vec::new();
            for _ in 0..3 {
                xyz.push(Value::from(f64::from_le_bytes(take(8)?.try_into().ok()?)));
            }
            tagged(5, Value::Array(xyz))
        }
        _ => return None,
    })
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_of_names_reads_as_the_later_nodes_keep_it() {
        // tag 0, two elements, each tag 5 + a name id.
        let v = node(3, br#"{"literalValue":"AAAAAAIAAAAAAAAABQAAAEgZAAAAAAAABQAAAEwZAAAAAAAA"}"#).expect("decodes");
        let want = rmpv::Value::Array(vec![Value::Array(vec![Value::Array(vec![
            Value::from(9),
            Value::Array(vec![
                Value::Array(vec![Value::Array(vec![Value::from(7), Value::from(6472u64)])]),
                Value::Array(vec![Value::Array(vec![Value::from(7), Value::from(6476u64)])]),
            ]),
        ])])]);
        assert_eq!(v, want);
    }

    #[test]
    fn a_step_and_broken_literals() {
        let step = node(2, br#"{"version":37,"functionName":"Extrude","stepName":"Extrusion 01","parameters":[5,6]}"#).expect("step");
        assert_eq!(step.as_array().and_then(|a| a[2].as_str()), Some("Extrude"));
        // A count far past the data, an unknown tag and a cut-off value are refused, not followed.
        assert!(node(3, br#"{"literalValue":"AAAAAP////////9/"}"#).is_none());
        assert!(node(3, br#"{"literalValue":"YwAAAA=="}"#).is_none());
        assert!(node(3, br#"{"literalValue":"AwAAAAAA"}"#).is_none());
    }
}
