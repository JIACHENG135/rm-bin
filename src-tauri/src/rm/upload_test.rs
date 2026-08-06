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

/// The `\x01` markers around each `.metadata` file's own uuid have to survive
/// round-tripping through the store even when a sibling document's
/// `visibleName` contains stray characters — everything except a raw
/// control byte, which JSON itself already forbids unescaped.
#[test]
fn snapshot_parsing_splits_entries_and_reads_their_fields() {
    let text = format!(
        "\u{1}{}\n{}\n\u{1}{}\n{}\n",
        "doc-uuid",
        r#"{"parent":"folder-uuid","visibleName":"my doc","type":"DocumentType"}"#,
        "folder-uuid",
        r#"{"parent":"","visibleName":"我的截图","type":"CollectionType"}"#,
    );
    let entries = upload::parse_snapshot(&text);
    assert_eq!(entries.len(), 2);

    let doc = entries.iter().find(|e| e.uuid == "doc-uuid").unwrap();
    assert_eq!(doc.parent, "folder-uuid");
    assert_eq!(doc.visible_name, "my doc");
    assert!(!doc.is_collection);

    let folder = entries.iter().find(|e| e.uuid == "folder-uuid").unwrap();
    assert_eq!(folder.parent, "");
    assert_eq!(folder.visible_name, "我的截图");
    assert!(folder.is_collection);
}

/// An existing top-level folder with the right name is reused rather than
/// duplicated — otherwise every upload into "工作笔记" would mint a new,
/// identically named folder next to the last one.
#[test]
fn resolve_folder_reuses_an_existing_top_level_collection() {
    let snap = upload::parse_snapshot(&format!(
        "\u{1}{}\n{}\n",
        "folder-uuid", r#"{"parent":"","visibleName":"工作笔记","type":"CollectionType"}"#
    ));
    let (uuid, entry) = upload::resolve_folder(&snap, "工作笔记", 0);
    assert_eq!(uuid, "folder-uuid");
    assert!(entry.is_none(), "must not recreate a folder that already exists");
}

/// No matching folder means one gets minted, as a `.metadata` entry the
/// caller writes alongside the document in the same ssh session.
#[test]
fn resolve_folder_creates_one_when_missing() {
    let (uuid, entry) = upload::resolve_folder(&[], "网购订单", 0);
    assert!(!uuid.is_empty());
    let entry = entry.expect("a new folder needs a metadata entry");
    assert_eq!(entry.name, format!("{uuid}.metadata"));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8(entry.bytes).unwrap()).unwrap();
    assert_eq!(v["type"], "CollectionType");
    assert_eq!(v["visibleName"], "网购订单");
}

/// The list handed to Gemini has to be exactly what `resolve_folder` can
/// match: a nested folder or a trashed one offered as a choice would come
/// back as a suggestion that mints a duplicate at the root instead of
/// reusing the folder the user was actually looking at.
#[test]
fn top_level_folders_are_the_ones_resolve_folder_can_reuse() {
    let snap = upload::parse_snapshot(&format!(
        "\u{1}{}\n{}\n\u{1}{}\n{}\n\u{1}{}\n{}\n\u{1}{}\n{}\n\u{1}{}\n{}\n",
        "root-b",
        r#"{"parent":"","visibleName":"移民材料","type":"CollectionType"}"#,
        "root-a",
        r#"{"parent":"","visibleName":"算法题解","type":"CollectionType"}"#,
        "nested",
        r#"{"parent":"root-a","visibleName":"背包问题","type":"CollectionType"}"#,
        "trashed",
        r#"{"parent":"trash","visibleName":"旧文件夹","type":"CollectionType"}"#,
        "a-doc",
        r#"{"parent":"","visibleName":"某份文档","type":"DocumentType"}"#,
    ));

    // Sorted, so the prompt reads the same way twice for the same library.
    assert_eq!(upload::top_level_folders(&snap), vec!["移民材料", "算法题解"]);

    for name in upload::top_level_folders(&snap) {
        let (_, minted) = upload::resolve_folder(&snap, &name, 0);
        assert!(minted.is_none(), "{name} was offered but would create a duplicate");
    }
}

/// A folder whose name is blank would be offered as an empty bullet and, if
/// echoed back, read as "no folder" by the caller.
#[test]
fn blank_folder_names_are_not_offered() {
    let snap = upload::parse_snapshot(&format!(
        "\u{1}{}\n{}\n\u{1}{}\n{}\n",
        "blank",
        r#"{"parent":"","visibleName":"   ","type":"CollectionType"}"#,
        "real",
        r#"{"parent":"","visibleName":"面试面经","type":"CollectionType"}"#,
    ));
    assert_eq!(upload::top_level_folders(&snap), vec!["面试面经"]);
}

/// Two documents Gemini names identically in the same folder must not
/// collide — the second gets a numbered suffix instead of overwriting the
/// first in the document list.
#[test]
fn dedupe_name_numbers_collisions_within_the_same_folder() {
    let snap = upload::parse_snapshot(&format!(
        "\u{1}{}\n{}\n\u{1}{}\n{}\n",
        "a",
        r#"{"parent":"folder","visibleName":"发货单","type":"DocumentType"}"#,
        "b",
        r#"{"parent":"folder","visibleName":"发货单 (2)","type":"DocumentType"}"#,
    ));
    assert_eq!(upload::dedupe_name(&snap, "folder", "发货单"), "发货单 (3)");
    assert_eq!(upload::dedupe_name(&snap, "folder", "收据"), "收据");
    // A same-named document in a *different* folder is not a collision.
    assert_eq!(upload::dedupe_name(&snap, "other-folder", "发货单"), "发货单");
}
