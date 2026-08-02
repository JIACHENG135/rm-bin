//! Round-trip the `.rm` writer through an independent parser.
//!
//! Everything here writes bytes for a format nobody documents, so "it
//! compiles" means nothing. `remarkable_lines` is a third-party reader with
//! no shared code with `rmfile.rs`; if it can read back what we wrote, and
//! reads the same shapes out of a file the tablet itself produced, the
//! encoding is right for reasons independent of our own understanding of it.
//! It is a dev-dependency only and never ships in the app.

use super::rmfile::{self, Point};
use remarkable_lines::v6::block::Block;
use remarkable_lines::RemarkableFile;

fn stroke(pts: &[(f32, f32)]) -> Vec<Point> {
    pts.iter().map(|&(x, y)| Point { x, y }).collect()
}

fn lines(bytes: &[u8]) -> Vec<remarkable_lines::v6::scene_item::line::Line> {
    let file = RemarkableFile::read(bytes).expect("parser rejected the file");
    let RemarkableFile::V6 { blocks, .. } = file else {
        panic!("not parsed as v6");
    };
    blocks
        .into_iter()
        .filter_map(|b| match b {
            Block::SceneLineItem(item) => item.item.value,
            _ => None,
        })
        .collect()
}

#[test]
fn writes_a_page_an_independent_parser_can_read() {
    let input = [
        stroke(&[(-700.0, 120.0), (700.0, 130.0), (700.0, 900.0)]),
        stroke(&[(-10.5, 2000.0), (0.0, 2010.25)]),
    ];
    let bytes = rmfile::page(&input);

    assert!(bytes.starts_with(b"reMarkable .lines file, version=6"));

    let got = lines(&bytes);
    assert_eq!(got.len(), input.len(), "line count");
    for (i, (line, want)) in got.iter().zip(&input).enumerate() {
        assert_eq!(line.points.len(), want.len(), "line {i} point count");
        for (p, w) in line.points.iter().zip(want) {
            // f32 in, f32 out — these should be exact, not merely close.
            assert_eq!((p.x, p.y), (w.x, w.y), "line {i} coordinates");
        }
    }
}

/// Strokes are a linked list, not an array: each line carries the id of the
/// one before it. Get that wrong and the tablet still opens the page, but the
/// strokes come out in an arbitrary order or vanish — so check the chain
/// explicitly rather than trusting that parsing succeeded.
#[test]
fn strokes_are_chained_in_drawing_order() {
    let input: Vec<_> = (0..5)
        .map(|i| stroke(&[(i as f32 * 10.0, 100.0), (i as f32 * 10.0, 200.0)]))
        .collect();
    let bytes = rmfile::page(&input);

    let RemarkableFile::V6 { blocks, .. } = RemarkableFile::read(&bytes[..]).unwrap() else {
        panic!("not v6");
    };
    let items: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::SceneLineItem(item) => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(items.len(), input.len());

    let mut expected_left = None;
    for (i, item) in items.iter().enumerate() {
        if let Some(prev) = expected_left {
            assert_eq!(item.item.left_id, prev, "line {i} does not follow line {}", i - 1);
        }
        expected_left = Some(item.item.item_id);
    }
}

/// A stroke of fewer than two points draws nothing and would only add a
/// degenerate item to the page.
#[test]
fn degenerate_strokes_are_dropped() {
    let input = [stroke(&[(0.0, 0.0)]), stroke(&[(1.0, 1.0), (2.0, 2.0)]), stroke(&[])];
    assert_eq!(lines(&rmfile::page(&input)).len(), 1);
}

/// Guards the template: it must stay a *bare* page. If a future copy is taken
/// from a tablet without stripping the trailing stub line, every page rm-bin
/// writes would carry a phantom stroke.
#[test]
fn the_template_contributes_no_strokes() {
    assert!(lines(&rmfile::page(&[])).is_empty());
}

/// The author UUID appears in both the page and the notebook's `.content`,
/// and the tablet expects them to agree.
#[test]
fn author_uuid_matches_between_page_and_content() {
    let uuid = rmfile::uuid_string(&rmfile::AUTHOR_UUID);
    assert!(rmfile::content("page-uuid", 1).contains(&uuid));
    assert!(rmfile::page(&[]).windows(16).any(|w| w == rmfile::AUTHOR_UUID));
}

/// The reference file the whole implementation was written against, kept as a
/// control: if this stops parsing, the parser changed under us and the
/// round-trip tests above are no longer evidence of anything.
#[test]
fn reference_file_from_a_real_tablet_still_parses() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/reference-page.rm");
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("skipping: no reference file at {path}");
        return;
    };
    let got = lines(&bytes);
    assert!(!got.is_empty(), "reference page has no strokes");
    let pts: usize = got.iter().map(|l| l.points.len()).sum();
    assert!(pts > 100, "reference page has only {pts} points");
}

/// Build a whole notebook from an image, on disk, ready to be copied to a
/// tablet. Run by hand:
///
///     RM_IMG=<path> cargo test --release --lib write_notebook -- --ignored --nocapture
#[test]
#[ignore]
fn write_notebook() {
    use super::{device, draw};
    let img = std::env::var("RM_IMG").expect("set RM_IMG");
    let calib = device::PAPER_PRO;
    let strokes = draw::page(&img, &calib).unwrap().strokes;
    let pts: usize = strokes.iter().map(|s| s.len()).sum();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let ms = now.as_millis();
    let seed = now.as_nanos();
    let id = |n: u128| {
        let h = format!("{:032x}", seed.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(n));
        format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    };
    let (doc, page_id) = (id(1), id(2));

    let out = std::env::temp_dir().join("rmbin-notebook");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(out.join(&doc)).unwrap();
    std::fs::write(out.join(format!("{doc}.metadata")), rmfile::metadata("RM Bin", ms)).unwrap();
    std::fs::write(out.join(format!("{doc}.content")), rmfile::content(&page_id, ms)).unwrap();
    std::fs::write(out.join(&doc).join(format!("{page_id}.rm")), rmfile::page(&strokes)).unwrap();

    // Prove it before it ever reaches a tablet.
    let written = std::fs::read(out.join(&doc).join(format!("{page_id}.rm"))).unwrap();
    assert_eq!(lines(&written).len(), strokes.iter().filter(|s| s.len() >= 2).count());

    println!("{} strokes, {pts} points -> {}", strokes.len(), out.display());
    println!("doc={doc}");
}
