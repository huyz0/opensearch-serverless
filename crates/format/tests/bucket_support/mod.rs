//! A test-only encoder for bucket manifests, written from the Java's
//! `CompactManifestFormat.body` and `BucketManifest.toBytes`. No Java process
//! wrote what it produces (docs/specs/bucket-manifest-reader/VERIFIED.md).

// ARITH: sizes here are small test values; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, dead_code, clippy::disallowed_methods)]

use std::collections::BTreeMap;

pub const MAGIC: &[u8; 4] = b"OSBM";

#[derive(Clone)]
pub enum Loc {
    Node {
        node: &'static str,
        epoch: u64,
        ordinal: u64,
        offset: u64,
        length: u64,
    },
    Pack {
        prefix: &'static str,
        offset: u64,
        length: u64,
    },
    Prefix(&'static str),
    Raw(&'static str),
}

#[derive(Clone, Copy)]
pub enum Len {
    None,
    OfRange,
    Given(u64),
}

#[derive(Clone)]
pub struct File {
    pub name: &'static str,
    pub loc: Loc,
    pub len: Len,
}

#[derive(Clone)]
pub struct Commit {
    pub term: u64,
    pub writer: Option<&'static str>,
    pub wal_ordinal: i64,
    pub stage: i64,
    pub files: Vec<File>,
    pub extra: Vec<(&'static str, u64)>,
    pub digest: Option<&'static str>,
}

#[derive(Clone)]
pub struct Member {
    pub uuid: String,
    pub index: String,
    pub version: u64,
    pub commit: Option<Commit>,
}

pub fn vlong(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub fn zlong(out: &mut Vec<u8>, v: i64) {
    let mut z = ((v << 1) ^ (v >> 63)) as u64;
    while z >= 0x80 {
        out.push((z & 0x7F) as u8 | 0x80);
        z >>= 7;
    }
    out.push(z as u8);
}

/// `StreamOutput.writeString`: the character count, then each UTF-16 unit.
pub fn string(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    vlong(out, units.len() as u64);
    for c in units {
        if c <= 0x7F {
            out.push(c as u8);
        } else if c > 0x7FF {
            out.extend([
                0xE0 | (c >> 12) as u8,
                0x80 | ((c >> 6) & 0x3F) as u8,
                0x80 | (c & 0x3F) as u8,
            ]);
        } else {
            out.extend([0xC0 | (c >> 6) as u8, 0x80 | (c & 0x3F) as u8]);
        }
    }
}

#[derive(Default)]
struct Strings {
    ids: BTreeMap<String, usize>,
    table: Vec<String>,
}

impl Strings {
    fn id(&mut self, s: &str) -> u64 {
        if let Some(id) = self.ids.get(s) {
            return *id as u64;
        }
        self.table.push(s.to_owned());
        self.ids.insert(s.to_owned(), self.table.len() - 1);
        (self.table.len() - 1) as u64
    }
}

fn split(name: &str) -> (&str, &str) {
    name.find('.').map_or((name, ""), |dot| name.split_at(dot))
}

fn commit(c: &Commit, strings: &mut Strings, out: &mut Vec<u8>) {
    vlong(out, c.term);
    vlong(out, c.writer.map_or(0, |w| strings.id(w) + 1));
    vlong(out, (c.wal_ordinal + 1) as u64);
    vlong(out, (c.stage + 1) as u64);
    let mut files = c.files.clone();
    files.sort_by_key(|f| f.name);
    let mut packs: Vec<String> = Vec::new();
    let pack_text = |loc: &Loc| match loc {
        Loc::Node {
            node,
            epoch,
            ordinal,
            ..
        } => Some(format!("@{node}/e={epoch:020}/{ordinal:020}")),
        Loc::Pack { prefix, .. } => Some((*prefix).to_owned()),
        _ => None,
    };
    for f in &files {
        if let Some(p) = pack_text(&f.loc)
            && !packs.contains(&p)
        {
            packs.push(p);
        }
    }
    vlong(out, packs.len() as u64);
    let (mut epoch, mut ordinal) = (0i64, 0i64);
    for p in &packs {
        match p.strip_prefix('@').and_then(|r| r.split_once('/')) {
            Some((node, rest)) if rest.starts_with("e=") => {
                let (e, o) = rest[2..].split_once('/').unwrap();
                let (e, o) = (e.parse::<i64>().unwrap(), o.parse::<i64>().unwrap());
                vlong(out, strings.id(node) << 1 | 1);
                zlong(out, e - epoch);
                zlong(out, o - ordinal);
                (epoch, ordinal) = (e, o);
            }
            _ => vlong(out, strings.id(p) << 1),
        }
    }
    vlong(out, files.len() as u64);
    for f in &files {
        let (stem, ext) = split(f.name);
        vlong(out, strings.id(stem));
        vlong(out, strings.id(ext));
        let range = match &f.loc {
            Loc::Node { offset, length, .. } | Loc::Pack { offset, length, .. } => {
                Some((*offset, *length))
            }
            _ => None,
        };
        let length_tag = match f.len {
            Len::None => 0,
            Len::OfRange => 1,
            Len::Given(_) => 2,
        };
        let location_tag = match &f.loc {
            Loc::Prefix(_) => 0,
            Loc::Node { .. } | Loc::Pack { .. } => 1,
            Loc::Raw(_) => 2,
        };
        vlong(out, location_tag << 2 | length_tag);
        match &f.loc {
            Loc::Prefix(p) | Loc::Raw(p) => vlong(out, strings.id(p)),
            other => {
                let at = packs
                    .iter()
                    .position(|p| Some(p) == pack_text(other).as_ref())
                    .unwrap();
                vlong(out, at as u64);
                vlong(out, range.unwrap().0);
                vlong(out, range.unwrap().1);
            }
        }
        if let Len::Given(n) = f.len {
            vlong(out, n);
        }
    }
    vlong(out, c.extra.len() as u64);
    for (name, length) in &c.extra {
        string(out, name);
        vlong(out, *length);
    }
    match c.digest {
        None => vlong(out, 0),
        Some(d) => {
            vlong(out, d.len() as u64);
            out.extend(d.as_bytes());
        }
    }
}

/// The uncompressed body, as `CompactManifestFormat.body` writes it.
pub fn body(bucket: &str, term: u64, members: &[Member]) -> Vec<u8> {
    let mut members: Vec<&Member> = members.iter().collect();
    members.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    let mut strings = Strings::default();
    let entries: Vec<Vec<u8>> = members
        .iter()
        .map(|m| {
            let mut e = Vec::new();
            string(&mut e, &m.index);
            vlong(&mut e, m.version);
            e.push(u8::from(m.commit.is_some()));
            if let Some(c) = &m.commit {
                commit(c, &mut strings, &mut e);
            }
            e
        })
        .collect();
    let mut out = Vec::new();
    string(&mut out, bucket);
    vlong(&mut out, term);
    vlong(&mut out, strings.table.len() as u64);
    for s in &strings.table {
        string(&mut out, s);
    }
    vlong(&mut out, entries.len() as u64);
    for (m, e) in members.iter().zip(&entries) {
        string(&mut out, &m.uuid);
        vlong(&mut out, e.len() as u64);
    }
    for e in entries {
        out.extend(e);
    }
    out
}

pub fn frame(codec: u8, raw_length: usize, packed: &[u8]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend([2, codec]);
    out.extend((raw_length as u32).to_be_bytes());
    out.extend(packed);
    out
}

pub const NONE: u8 = 0;
pub const DEFLATE: u8 = 1;
pub const ZSTD: u8 = 2;
pub const LZ4: u8 = 3;

pub fn compact(codec: u8, bucket: &str, term: u64, members: &[Member]) -> Vec<u8> {
    let raw = body(bucket, term, members);
    let packed = match codec {
        NONE => raw.clone(),
        DEFLATE => miniz_oxide::deflate::compress_to_vec(&raw, 1),
        ZSTD => {
            let mut out = vec![0u8; zstd_safe::compress_bound(raw.len())];
            let n = zstd_safe::compress(out.as_mut_slice(), &raw, 1).unwrap();
            out.truncate(n);
            out
        }
        LZ4 => lz4(&raw),
        other => panic!("no codec {other}"),
    };
    frame(codec, raw.len(), &packed)
}

/// Lucene's LZ4 block format, greedy over 4-byte windows, with matches that
/// may overlap what they copy; the last bytes are literals.
pub fn lz4(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut table: BTreeMap<[u8; 4], usize> = BTreeMap::new();
    let (mut at, mut anchor) = (0usize, 0usize);
    let limit = raw.len().saturating_sub(8);
    while at < limit {
        let key: [u8; 4] = raw[at..at + 4].try_into().unwrap();
        let candidate = table.insert(key, at);
        match candidate {
            Some(prev) if at - prev < 0x10000 && raw[prev..prev + 4] == key => {
                let mut len = 4;
                while at + len < raw.len() - 5 && raw[prev + len] == raw[at + len] {
                    len += 1;
                }
                sequence(&mut out, &raw[anchor..at], Some((at - prev, len)));
                at += len;
                anchor = at;
            }
            _ => at += 1,
        }
    }
    sequence(&mut out, &raw[anchor..], None);
    out
}

fn sequence(out: &mut Vec<u8>, literals: &[u8], matched: Option<(usize, usize)>) {
    let lit = literals.len();
    let m = matched.map_or(0, |(_, len)| len - 4);
    out.push(((lit.min(15) as u8) << 4) | m.min(15) as u8);
    extend(out, lit);
    out.extend(literals);
    if let Some((back, _)) = matched {
        out.extend([(back & 0xFF) as u8, (back >> 8) as u8]);
        extend(out, m);
    }
}

fn extend(out: &mut Vec<u8>, mut n: usize) {
    if n >= 15 {
        n -= 15;
        while n >= 255 {
            out.push(255);
            n -= 255;
        }
        out.push(n as u8);
    }
}

/// `BucketManifest.toBytes()`: the JSON form.
pub fn json(
    bucket: &str,
    term: u64,
    members: &[(&str, &str, Option<u64>, Option<&str>)],
) -> Vec<u8> {
    let mut out = format!("{{\"bucket\":\"{bucket}\",\"term\":{term},\"members\":{{");
    for (i, (uuid, index, version, commit)) in members.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("\"{uuid}\":{{\"index\":\"{index}\""));
        if let Some(v) = version {
            out.push_str(&format!(",\"version\":{v}"));
        }
        if let Some(c) = commit {
            out.push_str(&format!(",\"commit\":{c}"));
        }
        out.push('}');
    }
    out.push_str("}}");
    out.into_bytes()
}

/// A realistic commit: node packs in two epochs, a segment blob range, a
/// whole-blob file with no length, a raw-tagged location, a writer.
pub fn sample_commit() -> Commit {
    Commit {
        term: 7,
        writer: Some("tZ9iGQAAQACwLK7Z_____w"),
        wal_ordinal: 41,
        stage: 3,
        files: vec![
            File {
                name: "_0.cfe",
                loc: Loc::Node {
                    node: "nodeA",
                    epoch: 1_794_710_559_389,
                    ordinal: 4,
                    offset: 0,
                    length: 40,
                },
                len: Len::OfRange,
            },
            File {
                name: "_0.cfs",
                loc: Loc::Node {
                    node: "nodeA",
                    epoch: 1_794_710_559_389,
                    ordinal: 4,
                    offset: 40,
                    length: 900,
                },
                len: Len::OfRange,
            },
            File {
                name: "_0.si",
                loc: Loc::Node {
                    node: "nodeA",
                    epoch: 1_794_710_559_390,
                    ordinal: 2,
                    offset: 0,
                    length: 7,
                },
                len: Len::Given(7),
            },
            File {
                name: "_1.cfe",
                loc: Loc::Pack {
                    prefix: "t=3/s1",
                    offset: 5,
                    length: 11,
                },
                len: Len::OfRange,
            },
            File {
                name: "segments_4",
                loc: Loc::Prefix("t=3"),
                len: Len::None,
            },
            File {
                name: "_1.liv",
                loc: Loc::Prefix("t=3"),
                len: Len::Given(19),
            },
            File {
                name: "write.lock",
                loc: Loc::Raw("t=3/odd#x:y"),
                len: Len::None,
            },
        ],
        extra: vec![("ghost.bin", 5)],
        digest: Some("{\"fields\":{}}"),
    }
}
