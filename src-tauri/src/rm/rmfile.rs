//! Writing reMarkable `.rm` "lines" files, format version 6.
//!
//! The other half of `draw.rs`: instead of replaying strokes through the pen
//! digitizer, hand the tablet a finished page. No toolbar to blunder into, no
//! taper artifacts from xochitl's own stroke rendering, exact control of pen
//! and colour, and it lands on a *new* page instead of over whatever was
//! open. What it costs is the thing the pen replay was for — a file appears
//! all at once, so there is no progress for the window to follow.
//!
//! The format is undocumented. This is written against two sources, both
//! cross-checked byte for byte against files pulled off a real Paper Pro
//! (firmware 3.2x): the Kaitai spec at
//! <https://github.com/YakBarber/remarkable_file_format> and the reference
//! files themselves. The fixed scaffolding every page begins with is not
//! reconstructed at all — `page_template.bin` is a real empty page with its
//! author UUID blanked, and we append line blocks to it. Less to get wrong,
//! and it stays correct through firmware revisions that only touch parts we
//! don't synthesise.
//!
//! # Layout
//!
//! ```text
//! 43 bytes  "reMarkable .lines file, version=6" padded with spaces
//! blocks    u32 len | u8 0 | u8 min_ver | u8 cur_ver | u8 type | body[len]
//! ```
//!
//! Bodies use a tagged encoding: one byte of `field_index << 4 | type`, then
//! the value. Types seen here are `4` (u32), `8` (f64), `c` (length-prefixed
//! subblock) and `f` (CrdtId: a u8 then a LEB128 varint).

use std::fmt::Write as _;

/// A real empty page from a Paper Pro: the 43-byte header plus the eight
/// scaffolding blocks (AuthorIds, MigrationInfo, PageInfo, SceneInfo,
/// SceneTree, two TreeNodes and the SceneGroupItem that is layer 1). Its
/// trailing empty-line stub is deliberately not included, so line blocks can
/// simply be appended.
const TEMPLATE: &[u8] = include_bytes!("page_template.bin");
/// Where the author UUID sits inside `TEMPLATE`, zeroed in the committed copy.
const AUTHOR_UUID_AT: usize = 0x3a;
/// rm-bin's author UUID, stamped into both the page and the notebook's
/// `.content` — the tablet expects the two to agree.
pub const AUTHOR_UUID: [u8; 16] = [
    0x72, 0x6d, 0x62, 0x69, 0x6e, 0x00, 0x4d, 0x00, 0x8a, 0x00, 0x72, 0x6d, 0x62, 0x69, 0x6e, 0x01,
];

/// The layer every line hangs off — the SceneGroupItem in `TEMPLATE`.
const LAYER: CrdtId = CrdtId(0x00, 0x0b);
/// First line id. Matches what the tablet itself uses on a fresh page, and
/// sits clear of the ids `TEMPLATE` already spends on its scaffolding.
const FIRST_LINE: CrdtId = CrdtId(0x01, 0x10);

/// Pen 17 is the one xochitl's own injected strokes came out as; colour 0 is
/// black. The f64 is the brush scale, and `POINT_WIDTH` the per-point
/// rendered width — both copied from reference files rather than guessed,
/// since nothing documents their units.
const PEN_TYPE: u32 = 17;
const PEN_COLOR: u32 = 0;
const BRUSH_SCALE: f64 = 2.0;
const POINT_WIDTH: u16 = 16;
const POINT_PRESSURE: u8 = 0x80;

/// A CRDT identifier: an author-ish byte plus a per-author counter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CrdtId(pub u8, pub u64);

impl CrdtId {
    const NONE: CrdtId = CrdtId(0, 0);

    fn write(&self, out: &mut Vec<u8>) {
        out.push(self.0);
        varint(out, self.1);
    }
}

/// LEB128, as the format uses for the second half of a CrdtId.
fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn tag(out: &mut Vec<u8>, index: u8, ty: u8) {
    out.push(index << 4 | ty);
}

fn tagged_u32(out: &mut Vec<u8>, index: u8, v: u32) {
    tag(out, index, 4);
    out.extend_from_slice(&v.to_le_bytes());
}

fn tagged_f64(out: &mut Vec<u8>, index: u8, v: f64) {
    tag(out, index, 8);
    out.extend_from_slice(&v.to_le_bytes());
}

fn tagged_id(out: &mut Vec<u8>, index: u8, id: CrdtId) {
    tag(out, index, 0xf);
    id.write(out);
}

/// A tagged subblock whose u32 length is only known once its body is written.
fn tagged_block(out: &mut Vec<u8>, index: u8, body: impl FnOnce(&mut Vec<u8>)) {
    tag(out, index, 0xc);
    let len_at = out.len();
    out.extend_from_slice(&[0; 4]);
    let start = out.len();
    body(out);
    let len = (out.len() - start) as u32;
    out[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
}

fn block(
    out: &mut Vec<u8>,
    min_ver: u8,
    cur_ver: u8,
    block_type: u8,
    body: impl FnOnce(&mut Vec<u8>),
) {
    let len_at = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&[0, min_ver, cur_ver, block_type]);
    let start = out.len();
    body(out);
    let len = (out.len() - start) as u32;
    out[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
}

/// A point in page coordinates: x measured from the *centre* of the page, y
/// from its *top*, both in screen pixels (the two are 1:1 — confirmed by
/// measuring a drawing of known placement out of a file the tablet wrote).
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// Serialise `strokes` as a single-layer v6 page.
pub fn page(strokes: &[Vec<Point>]) -> Vec<u8> {
    let mut out = TEMPLATE.to_vec();
    out[AUTHOR_UUID_AT..AUTHOR_UUID_AT + 16].copy_from_slice(&AUTHOR_UUID);

    // Lines form a linked list: each carries the id of the one before it, and
    // the first carries none. Drawing order is this chain, not file order.
    let mut previous = CrdtId::NONE;
    for (i, stroke) in strokes.iter().enumerate() {
        if stroke.len() < 2 {
            continue;
        }
        let id = CrdtId(FIRST_LINE.0, FIRST_LINE.1 + i as u64);
        block(&mut out, 2, 2, 0x05, |b| {
            tagged_id(b, 1, LAYER);
            tagged_id(b, 2, id);
            tagged_id(b, 3, previous);
            tagged_id(b, 4, CrdtId::NONE);
            tagged_u32(b, 5, 0); // not deleted
            tagged_block(b, 6, |b| {
                b.push(0x03); // undocumented, constant in every reference file
                tagged_u32(b, 1, PEN_TYPE);
                tagged_u32(b, 2, PEN_COLOR);
                tagged_f64(b, 3, BRUSH_SCALE);
                tagged_u32(b, 4, 0);
                tagged_block(b, 5, |b| {
                    for p in stroke {
                        b.extend_from_slice(&p.x.to_le_bytes());
                        b.extend_from_slice(&p.y.to_le_bytes());
                        b.extend_from_slice(&0u16.to_le_bytes()); // speed
                        b.extend_from_slice(&POINT_WIDTH.to_le_bytes());
                        b.push(0); // direction
                        b.push(POINT_PRESSURE);
                    }
                });
                tagged_id(b, 6, CrdtId(0x00, 0x01));
            });
        });
        previous = id;
    }
    out
}

/// The notebook wrapper xochitl needs alongside the page: `<uuid>.metadata`
/// and `<uuid>.content`. Hand-built rather than templated because they're
/// plain JSON and the fields that matter are all ones we set.
pub fn metadata(name: &str, now_ms: u128) -> String {
    format!(
        r#"{{
    "createdTime": "{now_ms}",
    "lastModified": "{now_ms}",
    "lastOpened": "{now_ms}",
    "lastOpenedPage": 0,
    "new": false,
    "parent": "",
    "pinned": false,
    "source": "",
    "type": "DocumentType",
    "visibleName": "{name}"
}}
"#
    )
}

pub fn content(page_uuid: &str, now_ms: u128) -> String {
    let author = uuid_string(&AUTHOR_UUID);
    let mut s = String::new();
    let _ = write!(
        s,
        r#"{{
    "cPages": {{
        "lastOpened": {{ "timestamp": "1:1", "value": "{page_uuid}" }},
        "original": {{ "timestamp": "0:0", "value": -1 }},
        "pages": [
            {{
                "id": "{page_uuid}",
                "idx": {{ "timestamp": "1:2", "value": "ba" }},
                "modifed": "{now_ms}",
                "template": {{ "timestamp": "1:1", "value": "Blank" }}
            }}
        ],
        "uuids": [ {{ "first": "{author}", "second": 1 }} ]
    }},
    "coverPageNumber": -1,
    "customZoomCenterX": 0,
    "customZoomCenterY": 936,
    "customZoomOrientation": "portrait",
    "customZoomPageHeight": 1872,
    "customZoomPageWidth": 1404,
    "customZoomScale": 1,
    "documentMetadata": {{}},
    "extraMetadata": {{}},
    "fileType": "notebook",
    "fontName": "",
    "formatVersion": 2,
    "lineHeight": -1,
    "margins": 125,
    "orientation": "portrait",
    "pageCount": 1,
    "pageTags": [],
    "sizeInBytes": "0",
    "tags": [],
    "textAlignment": "justify",
    "textScale": 1,
    "zoomMode": "bestFit"
}}
"#
    );
    s
}

pub fn uuid_string(b: &[u8; 16]) -> String {
    let h = |r: &[u8]| r.iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        h(&b[0..4]),
        h(&b[4..6]),
        h(&b[6..8]),
        h(&b[8..10]),
        h(&b[10..16])
    )
}
