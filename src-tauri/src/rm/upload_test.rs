//! The parts of the upload that can be wrong without anything complaining.
//!
//! `install_files` itself needs a tablet, but everything it depends on being
//! right — that the remote script asks for exactly the bytes we send, in the
//! order we send them, and that it restarts xochitl only after every write —
//! is checkable here.

use super::upload::{self, Entry};

fn entries() -> Vec<Entry> {
    vec![
        Entry { name: "a.metadata".into(), bytes: vec![1, 2, 3] },
        Entry { name: "a.content".into(), bytes: vec![4, 5] },
        Entry { name: "a/p.rm".into(), bytes: vec![6, 7, 8, 9] },
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

/// The whole framing rests on this: files arrive down one stdin, and each
/// `dd` has to take exactly its own file's bytes. If a count and a payload
/// ever disagree, every file after it is shifted and the document is
/// silently garbage.
#[test]
fn the_script_asks_for_exactly_the_bytes_that_are_sent() {
    let files = entries();
    let script = upload::script_for(&files, &["a".into()]);
    assert_eq!(dd_sizes(&script), files.iter().map(|f| f.bytes.len()).collect::<Vec<_>>());
}

/// `set -e` is what keeps a half-written document from being followed by a
/// restart that publishes it, and the restart has to be the last thing.
#[test]
fn the_script_aborts_on_error_and_restarts_last() {
    let script = upload::script_for(&entries(), &["a".into()]);
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

#[test]
fn each_call_gets_its_own_identity() {
    let a = upload::uuid();
    let b = upload::uuid();
    assert_ne!(a, b);

    // 8-4-4-4-12 lowercase hex.
    for id in [&a, &b] {
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
}

/// A long filename shouldn't produce an unreadable entry in the document list.
#[test]
fn absurd_names_are_trimmed() {
    let long = format!("/tmp/{}.png", "x".repeat(500));
    assert!(upload::name_from_path(&long).chars().count() <= 60);
}
