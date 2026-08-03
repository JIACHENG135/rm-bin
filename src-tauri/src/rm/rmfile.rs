//! The two bits of `.metadata`/uuid formatting the PDF path still needs.
//!
//! This used to also write reMarkable's `.rm` v6 binary page format for the
//! pen-replay and notebook-writer modes; both were removed when rm-bin was
//! cut down to PDF-only, leaving just the notebook metadata JSON and the
//! uuid formatter `pdf.rs`'s ssh-fallback path still uses.

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
