use super::markdown;
use crate::rm::device::{PAPER_PRO, RM2};

/// Every stroke a plan produces should land inside the page it was laid out
/// for — a bound anything downstream can rely on without re-checking.
fn assert_within_page(preview: &[crate::rm::draw::PreviewStroke]) {
    // `PreviewStroke` is already clamped to 0..PREVIEW_UNITS by construction
    // (see draw.rs's `to_preview`), so this is really asserting the preview
    // pipeline didn't panic or silently drop everything.
    assert!(!preview.is_empty());
    for stroke in preview {
        assert!(stroke.len() >= 2, "a stroke with under two points is not a stroke");
    }
}

#[test]
fn plan_smoke() {
    let plan = markdown::plan("# Hello\n\nThis is a paragraph of body text.", &PAPER_PRO).unwrap();
    assert!(!plan.bytes.is_empty());
    assert!(plan.stroke_count() > 0);
    assert_within_page(&plan.preview);
}

#[test]
fn empty_markdown_is_an_error_not_a_blank_page() {
    // A blank plan would draw nothing and report success — the point of
    // an explicit error is that "there was nothing to draw" is visible to
    // whoever called this, the same way `draw::trace_and_order` refuses an
    // image with no traceable lines rather than silently no-op'ing.
    assert!(markdown::plan("", &PAPER_PRO).is_err());
    assert!(markdown::plan("   \n\n   ", &PAPER_PRO).is_err());
}

#[test]
fn mixed_cjk_and_latin_both_produce_ink() {
    // The whole point of splitting scripts at the tokenizer is that neither
    // side goes missing when they're mixed on one line.
    let plan = markdown::plan("Hello 世界, this is 混合 text.", &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() > 5, "mixed-script line produced suspiciously little ink");
}

#[test]
fn heading_and_paragraph_both_draw() {
    let plan = markdown::plan("# Title\n\nBody paragraph one.\n\nBody paragraph two.", &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() > 10);
}

#[test]
fn table_draws_grid_lines_plus_cell_text() {
    let md = "\
| A | B |
|---|---|
| one | two |
| three | four |
";
    let plan = markdown::plan(md, &PAPER_PRO).unwrap();
    // 2 columns -> 3 vertical rules, plus top/bottom/header rules: at least
    // 6 pure-grid strokes, on top of whatever the cell text traces to.
    assert!(plan.stroke_count() > 6);
}

#[test]
fn bullet_list_draws_markers_and_text() {
    let md = "- first item\n- second item\n- third item";
    let plan = markdown::plan(md, &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() > 3);
}

#[test]
fn ordered_list_numbers_increment() {
    // Not directly observable from strokes alone, but this at least proves
    // three items each produced their own marker + text rather than the
    // counter panicking or collapsing everything onto one line.
    let md = "1. alpha\n2. beta\n3. gamma";
    let plan = markdown::plan(md, &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() > 3);
}

#[test]
fn code_block_draws_an_indent_bar() {
    let md = "```\nfn main() {}\n```";
    let plan = markdown::plan(md, &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() >= 1);
}

#[test]
fn missing_local_image_is_skipped_not_fatal() {
    // No network fetch, and a bad path shouldn't take the rest of the
    // document down with it — matches diagram.rs's "drop the malformed
    // part, keep going" policy elsewhere in this codebase.
    let md = "# Title\n\n![alt](/no/such/file.png)\n\nStill here.";
    let plan = markdown::plan(md, &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() > 2);
}

#[test]
fn remote_image_url_is_skipped_not_fetched() {
    let md = "![alt](https://example.com/x.png)\n\nText after.";
    let plan = markdown::plan(md, &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() > 0);
}

#[test]
fn works_on_both_device_generations() {
    // Page size differs (1632x2154 vs 1404x1872) and so does event_size —
    // the layout is meant to be resolution-independent (BODY_FRAC scales
    // off screen_h), and `plan_from_page_strokes` is meant to encode
    // correctly either way.
    for calib in [&PAPER_PRO, &RM2] {
        let plan = markdown::plan("# Works everywhere\n\nBody text.", calib).unwrap();
        assert!(plan.stroke_count() > 0, "{:?}", calib.model);
    }
}

#[test]
fn very_long_document_stops_at_the_page_rather_than_erroring() {
    let mut md = String::new();
    for i in 0..200 {
        md.push_str(&format!("## Heading {i}\n\nSome body text for section {i}.\n\n"));
    }
    // Should not panic, hang, or fail — it should draw as much as fits and
    // stop, per `layout`'s `has_room` check.
    let plan = markdown::plan(&md, &PAPER_PRO).unwrap();
    assert!(plan.stroke_count() > 0);
}

/// Regression test for a real bug: `layout_table` used to lay each cell out
/// with `flow_text`, which advances the shared vertical cursor as a side
/// effect — fine for one block owning the whole cursor, wrong for several
/// cells that are supposed to share one row's top. The effect was every
/// cell after the first in a row landing one row lower than it should,
/// cascading through the rest of the table. A two-column, two-row table's
/// cells should all fall into two horizontal bands, not four staggered
/// ones.
#[test]
fn table_cells_in_one_row_share_a_vertical_band() {
    let md = "\
| Name | Score |
|------|-------|
| Alice | 92 |
| Bob | 77 |
";
    let strokes = markdown::debug_layout(md, &PAPER_PRO).unwrap();

    // Bucket every stroke by its vertical midpoint into distinct bands
    // (anything within one line-height of each other counts as the same
    // band) — a healthy 2-row table has exactly two bands of *cell text*
    // strokes, not up to four.
    let mut mids: Vec<f64> = strokes
        .iter()
        .map(|s| s.iter().map(|&(_, y)| y).sum::<f64>() / s.len() as f64)
        .collect();
    mids.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let band_gap = PAPER_PRO.screen_h / 46.0; // ~ one line height
    let mut bands = 1;
    for w in mids.windows(2) {
        if w[1] - w[0] > band_gap {
            bands += 1;
        }
    }
    // Two table rows plus whatever header-rule strokes land near them —
    // the point of this assertion is that it's small and flat, not that it
    // matches one exact number: the bug this guards against produced a
    // visibly staggered four-or-more-band table on the same input.
    assert!(bands <= 3, "expected ~2 row-bands, got {bands} distinct vertical bands: {mids:?}");
}

#[test]
fn page_and_plan_agree_on_stroke_count() {
    // Same layout function underneath (`layout`) — `plan_from_page_strokes`
    // and `page_from_page_strokes` are two different encodings of the same
    // strokes, so they should never disagree on how many there are.
    let md = "# Same strokes\n\nBoth outputs should carry the same drawing.";
    let plan = markdown::plan(md, &PAPER_PRO).unwrap();
    let page = markdown::page(md, &PAPER_PRO).unwrap();
    assert_eq!(plan.stroke_count(), page.preview.len());
}
