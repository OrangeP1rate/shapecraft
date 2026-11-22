//! Binary FBX 7.4 (what Blender, Unity, Unreal and the FBX SDK read; Blender won't take the ASCII form) and
//! VRML 2.0. One mesh model per body with its colour as a Phong material. Model space is Z-up millimetres: FBX
//! says so in GlobalSettings (UnitScaleFactor 0.1 cm), VRML gets a 0.001 scale to its metres.

use std::fmt::Write as _;

use super::mesh_out::ExportBody;

/// One FBX property value.
enum P<'a> {
    I32(i32),
    I64(i64),
    F64(f64),
    Str(&'a [u8]),
    Owned(Vec<u8>),
    F64s(Vec<f64>),
    I32s(Vec<i32>),
}

struct Node<'a> {
    name: &'static str,
    props: Vec<P<'a>>,
    children: Vec<Node<'a>>,
}

fn n<'a>(name: &'static str, props: Vec<P<'a>>, children: Vec<Node<'a>>) -> Node<'a> {
    Node { name, props, children }
}

/// A Properties70 entry: name, type, label, flags, values.
fn p70<'a>(name: &'a str, ty: &'a str, label: &'a str, values: Vec<P<'a>>) -> Node<'a> {
    let mut props = vec![P::Str(name.as_bytes()), P::Str(ty.as_bytes()), P::Str(label.as_bytes()), P::Str(b"")];
    props.extend(values);
    n("P", props, vec![])
}

fn write_prop(out: &mut Vec<u8>, p: &P) {
    let array = |out: &mut Vec<u8>, code: u8, count: usize, raw: Vec<u8>| {
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        out.push(code);
        out.extend_from_slice(&(count as u32).to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(z.len() as u32).to_le_bytes());
        out.extend_from_slice(&z);
    };
    match p {
        P::I32(v) => {
            out.push(b'I');
            out.extend_from_slice(&v.to_le_bytes());
        }
        P::I64(v) => {
            out.push(b'L');
            out.extend_from_slice(&v.to_le_bytes());
        }
        P::F64(v) => {
            out.push(b'D');
            out.extend_from_slice(&v.to_le_bytes());
        }
        P::Str(s) => {
            out.push(b'S');
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s);
        }
        P::Owned(s) => write_prop(out, &P::Str(s)),
        P::F64s(v) => array(out, b'd', v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
        P::I32s(v) => array(out, b'i', v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
    }
}

/// A node record (FBX < 7.5: u32 end offset, property count, property bytes, u8 name length), its children, and
/// the 13-byte null record that closes a node with children or without properties.
fn write_node(out: &mut Vec<u8>, node: &Node) {
    let start = out.len();
    out.extend_from_slice(&[0u8; 12]);
    out.push(node.name.len() as u8);
    out.extend_from_slice(node.name.as_bytes());
    let props_at = out.len();
    for p in &node.props {
        write_prop(out, p);
    }
    let props_len = (out.len() - props_at) as u32;
    for c in &node.children {
        write_node(out, c);
    }
    if !node.children.is_empty() || node.props.is_empty() {
        out.extend_from_slice(&[0u8; 13]);
    }
    let end = out.len() as u32;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&(node.props.len() as u32).to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&props_len.to_le_bytes());
}

/// "Name\x00\x01Class": how FBX names an object.
fn object_name(name: &str, class: &str) -> Vec<u8> {
    let mut v = name.as_bytes().to_vec();
    v.extend_from_slice(b"\x00\x01");
    v.extend_from_slice(class.as_bytes());
    v
}

/// The bodies as a binary FBX 7.4 file; None past what 32-bit offsets hold.
pub fn write_fbx(bodies: &[ExportBody]) -> Option<Vec<u8>> {
    let mut objects = Vec::new();
    let mut connections = Vec::new();
    // Ids only need to be unique and non-zero (0 is the scene root).
    let mut next = 1_000_000i64;
    let mut id = || {
        next += 1;
        next
    };
    for b in bodies {
        let (model, geometry, material) = (id(), id(), id());
        let m = &b.mesh;
        let verts: Vec<f64> = m.positions.iter().flat_map(|p| p.map(f64::from)).collect();
        let tris = m.indices.as_chunks::<3>().0;
        let index: Vec<i32> = tris.iter().flat_map(|t| [t[0] as i32, t[1] as i32, !(t[2] as i32)]).collect();
        let mut layers = vec![n(
            "LayerElement",
            vec![],
            vec![n("Type", vec![P::Str(b"LayerElementMaterial")], vec![]), n("TypedIndex", vec![P::I32(0)], vec![])],
        )];
        let mut geo = vec![
            n("Vertices", vec![P::F64s(verts)], vec![]),
            n("PolygonVertexIndex", vec![P::I32s(index)], vec![]),
            n("GeometryVersion", vec![P::I32(124)], vec![]),
        ];
        if m.normals.len() == m.positions.len() {
            let normals: Vec<f64> = tris.iter().flatten().flat_map(|&i| m.normals[i as usize].map(f64::from)).collect();
            geo.push(n(
                "LayerElementNormal",
                vec![P::I32(0)],
                vec![
                    n("Version", vec![P::I32(101)], vec![]),
                    n("Name", vec![P::Str(b"")], vec![]),
                    n("MappingInformationType", vec![P::Str(b"ByPolygonVertex")], vec![]),
                    n("ReferenceInformationType", vec![P::Str(b"Direct")], vec![]),
                    n("Normals", vec![P::F64s(normals)], vec![]),
                ],
            ));
            layers.push(n(
                "LayerElement",
                vec![],
                vec![n("Type", vec![P::Str(b"LayerElementNormal")], vec![]), n("TypedIndex", vec![P::I32(0)], vec![])],
            ));
        }
        geo.push(n(
            "LayerElementMaterial",
            vec![P::I32(0)],
            vec![
                n("Version", vec![P::I32(101)], vec![]),
                n("Name", vec![P::Str(b"")], vec![]),
                n("MappingInformationType", vec![P::Str(b"AllSame")], vec![]),
                n("ReferenceInformationType", vec![P::Str(b"IndexToDirect")], vec![]),
                n("Materials", vec![P::I32s(vec![0])], vec![]),
            ],
        ));
        let mut layer = vec![n("Version", vec![P::I32(100)], vec![])];
        layer.extend(layers);
        geo.push(n("Layer", vec![P::I32(0)], layer));
        objects.push(n("Geometry", vec![P::I64(geometry), P::Owned(object_name(b.name, "Geometry")), P::Str(b"Mesh")], geo));
        objects.push(n(
            "Model",
            vec![P::I64(model), P::Owned(object_name(b.name, "Model")), P::Str(b"Mesh")],
            vec![
                n("Version", vec![P::I32(232)], vec![]),
                n("Properties70", vec![], vec![]),
                n("Shading", vec![P::Str(b"Y")], vec![]),
                n("Culling", vec![P::Str(b"CullingOff")], vec![]),
            ],
        ));
        let [r, g, bl] = b.color.map(f64::from);
        let spec = f64::from(b.gloss) * 0.5;
        objects.push(n(
            "Material",
            vec![P::I64(material), P::Owned(object_name(b.name, "Material")), P::Str(b"")],
            vec![
                n("Version", vec![P::I32(102)], vec![]),
                n("ShadingModel", vec![P::Str(b"phong")], vec![]),
                n("MultiLayer", vec![P::I32(0)], vec![]),
                n(
                    "Properties70",
                    vec![],
                    vec![
                        p70("DiffuseColor", "Color", "", vec![P::F64(r), P::F64(g), P::F64(bl)]),
                        p70("SpecularColor", "Color", "", vec![P::F64(spec), P::F64(spec), P::F64(spec)]),
                        p70("Shininess", "double", "Number", vec![P::F64(10.0 + f64::from(b.gloss) * 90.0)]),
                        p70("ReflectionFactor", "double", "Number", vec![P::F64(f64::from(b.metal))]),
                    ],
                ),
            ],
        ));
        for (child, parent) in [(model, 0), (geometry, model), (material, model)] {
            connections.push(n("C", vec![P::Str(b"OO"), P::I64(child), P::I64(parent)], vec![]));
        }
    }
    let count = |ty: &'static str, k: usize| n("ObjectType", vec![P::Str(ty.as_bytes())], vec![n("Count", vec![P::I32(k as i32)], vec![])]);
    let top = vec![
        n(
            "FBXHeaderExtension",
            vec![],
            vec![
                n("FBXHeaderVersion", vec![P::I32(1003)], vec![]),
                n("FBXVersion", vec![P::I32(7400)], vec![]),
                n("Creator", vec![P::Str(b"ShapeCraft")], vec![]),
            ],
        ),
        n("Creator", vec![P::Str(b"ShapeCraft")], vec![]),
        n(
            "GlobalSettings",
            vec![],
            vec![
                n("Version", vec![P::I32(1000)], vec![]),
                n(
                    "Properties70",
                    vec![],
                    vec![
                        p70("UpAxis", "int", "Integer", vec![P::I32(2)]),
                        p70("UpAxisSign", "int", "Integer", vec![P::I32(1)]),
                        p70("FrontAxis", "int", "Integer", vec![P::I32(1)]),
                        p70("FrontAxisSign", "int", "Integer", vec![P::I32(-1)]),
                        p70("CoordAxis", "int", "Integer", vec![P::I32(0)]),
                        p70("CoordAxisSign", "int", "Integer", vec![P::I32(1)]),
                        p70("OriginalUpAxis", "int", "Integer", vec![P::I32(2)]),
                        p70("OriginalUpAxisSign", "int", "Integer", vec![P::I32(1)]),
                        p70("UnitScaleFactor", "double", "Number", vec![P::F64(0.1)]),
                        p70("OriginalUnitScaleFactor", "double", "Number", vec![P::F64(0.1)]),
                    ],
                ),
            ],
        ),
        n(
            "Definitions",
            vec![],
            vec![
                n("Version", vec![P::I32(100)], vec![]),
                n("Count", vec![P::I32(3 * bodies.len() as i32 + 1)], vec![]),
                count("GlobalSettings", 1),
                count("Model", bodies.len()),
                count("Geometry", bodies.len()),
                count("Material", bodies.len()),
            ],
        ),
        n("Objects", vec![], objects),
        n("Connections", vec![], connections),
    ];
    let mut out = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    out.extend_from_slice(&7400u32.to_le_bytes());
    for node in &top {
        write_node(&mut out, node);
        if out.len() > u32::MAX as usize {
            return None;
        }
    }
    out.extend_from_slice(&[0u8; 13]);
    // The footer the SDK writes: an id, padding to 16 bytes, the version again, and a closing magic.
    out.extend_from_slice(&[0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e]);
    out.extend(std::iter::repeat_n(0u8, (16 - out.len() % 16) % 16));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&7400u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 120]);
    out.extend_from_slice(&[0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b]);
    (out.len() <= u32::MAX as usize).then_some(out)
}

/// VRML 2.0 (`.wrl`): one Shape per body with its colour, in metres.
pub fn write_vrml(bodies: &[ExportBody]) -> String {
    let mut s = String::from("#VRML V2.0 utf8\n# ShapeCraft\nTransform {\n  scale 0.001 0.001 0.001\n  children [\n");
    for b in bodies {
        let name: String = b.name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
        let name = if name.starts_with(|c: char| c.is_ascii_digit()) || name.is_empty() { format!("B_{name}") } else { name };
        let [r, g, bl] = b.color;
        let _ = write!(
            s,
            "    DEF {name} Shape {{\n      appearance Appearance {{ material Material {{ diffuseColor {r:.4} {g:.4} {bl:.4} specularColor {sp:.3} {sp:.3} {sp:.3} shininess {sh:.3} }} }}\n      geometry IndexedFaceSet {{\n        solid FALSE\n        coord Coordinate {{ point [\n",
            sp = b.gloss * 0.5,
            sh = b.gloss
        );
        for p in &b.mesh.positions {
            let _ = writeln!(s, "          {} {} {},", p[0], p[1], p[2]);
        }
        s.push_str("        ] }\n");
        if b.mesh.normals.len() == b.mesh.positions.len() {
            s.push_str("        normal Normal { vector [\n");
            for v in &b.mesh.normals {
                let _ = writeln!(s, "          {} {} {},", v[0], v[1], v[2]);
            }
            s.push_str("        ] }\n        normalPerVertex TRUE\n");
        }
        s.push_str("        coordIndex [\n");
        for t in b.mesh.indices.as_chunks::<3>().0 {
            let _ = writeln!(s, "          {}, {}, {}, -1,", t[0], t[1], t[2]);
        }
        s.push_str("        ]\n      }\n    }\n");
    }
    s.push_str("  ]\n}\n");
    s
}
