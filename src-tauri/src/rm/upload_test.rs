//! The parts of the upload that can be wrong without anything complaining.
//!
//! `install` itself needs a tablet, but everything it depends on being right —
//! that the remote script asks for exactly the bytes we send, in the order we
//! send them, and that the names inside the notebook agree — is checkable
//! here, and each of these is a failure that would otherwise show up as a
//! notebook that quietly doesn't open.

use super::rmfile::Point;
use super::upload;

fn strokes() -> Vec<Vec<Point>> {
    vec![
        vec![
            Point { x: -300.0, y: 200.0 },
            Point { x: 300.0, y: 220.0 },
        ],
        vec![Point { x: 0.0, y: 400.0 }, Point { x: 10.0, y: 900.0 }],
    ]
}

/// Pull the `bs=` operands out of the script, in order.
fn dd_sizes(script: &str) -> Vec<usize> {
    script
        .lines()
        .filter(|l| l.trim_start().starts_with("dd "))
        .filter_map(|l| {
            l.split_whitespace()
                .find_map(|w| w.strip_prefix("bs="))
                .and_then(|n| n.parse().ok())
        })
        .collect()
}

/// The whole framing rests on this: three files arrive down one stdin, and
/// each `dd` has to take exactly its own file's bytes. If a count and a
/// payload ever disagree, every file after it is shifted and the notebook is
/// silently garbage.
#[test]
fn the_script_asks_for_exactly_the_bytes_that_are_sent() {
    let nb = upload::build("drawing", &strokes(), 1_700_000_000_000);
    assert_eq!(
        dd_sizes(&upload::script(&nb)),
        vec![nb.metadata.len(), nb.content.len(), nb.page_bytes.len()],
        "dd sizes must match the payloads, in the order install() writes them"
    );
    assert_eq!(
        nb.len(),
        nb.metadata.len() + nb.content.len() + nb.page_bytes.len()
    );
}

/// `set -e` is what keeps a half-written notebook from being followed by a
/// restart that publishes it, and the restart has to be the last thing.
#[test]
fn the_script_aborts_on_error_and_restarts_last() {
    let nb = upload::build("drawing", &strokes(), 1);
    let script = upload::script(&nb);
    assert!(script.starts_with("set -e"), "{script}");

    let lines: Vec<_> = script.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(*lines.last().unwrap(), "systemctl restart xochitl");
    let restart = lines.len() - 1;
    let last_write = lines
        .iter()
        .rposition(|l| l.trim_start().starts_with("dd "))
        .expect("no writes in script");
    assert!(last_write < restart, "restart must follow every write");
}

/// The tablet reads the page's own uuid out of `.content`; if the two names
/// disagree the notebook opens empty.
#[test]
fn the_page_is_named_the_same_in_the_content_file() {
    let nb = upload::build("drawing", &strokes(), 1);
    assert!(nb.content.contains(&nb.page));
    assert!(upload::script(&nb).contains(&format!("{}/{}.rm", nb.doc, nb.page)));
    assert_ne!(nb.doc, nb.page);
}

#[test]
fn each_notebook_gets_its_own_identity() {
    let a = upload::build("x", &strokes(), 1);
    let b = upload::build("x", &strokes(), 1);
    assert_ne!(a.doc, b.doc);
    assert_ne!(a.page, b.page);

    // 8-4-4-4-12 lowercase hex.
    for id in [&a.doc, &a.page] {
        let parts: Vec<_> = id.split('-').map(str::len).collect();
        assert_eq!(parts, vec![8, 4, 4, 4, 12], "{id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'), "{id}");
    }
}

/// The name is interpolated straight into `.metadata`'s JSON, so a filename
/// with a quote in it would otherwise write a file xochitl can't parse.
#[test]
fn names_come_from_the_file_and_cannot_break_the_metadata() {
    assert_eq!(upload::name_from_path("/tmp/sketch.png"), "sketch");
    assert_eq!(upload::name_from_path("/tmp/我的图.jpeg"), "我的图");
    // Nothing to take a name from, and a name that is only spaces: both have
    // to land on the fallback rather than an empty entry in the document list.
    assert_eq!(upload::name_from_path("/"), "RM Bin");
    assert_eq!(upload::name_from_path("/tmp/   .png"), "RM Bin");

    let nasty = upload::name_from_path("/tmp/a\"b\\c\nd.png");
    assert!(!nasty.contains(['"', '\\', '\n']), "{nasty}");

    let nb = upload::build(&nasty, &strokes(), 1);
    serde_json::from_str::<serde_json::Value>(&nb.metadata).expect("metadata must be valid JSON");
    serde_json::from_str::<serde_json::Value>(&nb.content).expect("content must be valid JSON");
}

/// A long filename shouldn't produce an unreadable entry in the document list.
#[test]
fn absurd_names_are_trimmed() {
    let long = format!("/tmp/{}.png", "x".repeat(500));
    assert!(upload::name_from_path(&long).chars().count() <= 60);
}
