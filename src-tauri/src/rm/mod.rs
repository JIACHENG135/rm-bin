//! Drawing an image onto a real reMarkable, by pretending to be its pen.
//!
//! The tablet has no import API worth using, but it does have a pen digitizer
//! at `/dev/input/eventN` that will happily accept events from anyone with
//! ssh. So: trace the image into strokes, encode them as raw `input_event`
//! records, and stream them in. This is the same trick rm-agent plays, except
//! rm-agent runs on the tablet writing to a local file, and rm-bin runs on
//! the Mac writing down an ssh pipe.

pub mod device;
pub mod draw;
/// Copied verbatim from rm-agent so fixes can be diffed straight across;
/// left un-idiomatic on purpose rather than drifting from the original.
#[allow(clippy::needless_range_loop, clippy::nonminimal_bool)]
pub mod imageproc;

#[cfg(test)]
#[path = "draw_test.rs"]
mod draw_test;

#[cfg(test)]
#[path = "imageproc_test.rs"]
mod imageproc_test;

/// Writing finished `.rm` pages instead of replaying the pen. `upload` puts
/// them on the tablet; which of the two paths runs is `Settings::mode`.
pub mod rmfile;
pub mod upload;

/// Painting the panel directly, with xochitl stopped — the only path that
/// puts the actual image up rather than something a pen could have drawn.
/// The device half lives in `../../../rmfb`.
pub mod screen;

/// Redrawing a photograph as line art before tracing it, because the tracer
/// needs lines and a photograph has none.
pub mod gemini;

#[cfg(test)]
#[path = "gemini_test.rs"]
mod gemini_test;

/// The image as a one-page PDF, posted to the tablet's own importer — the
/// only path that is the picture, is a document, and can be written on.
pub mod pdf;

#[cfg(test)]
#[path = "pdf_test.rs"]
mod pdf_test;

/// Contour tracing for flat, graphic images — a logo, an icon, a UI
/// screenshot — where a filled region's *outline* is the meaningful shape
/// and a skeleton centreline would find nothing. See the module doc.
pub mod vectorize;

#[cfg(test)]
#[path = "vectorize_test.rs"]
mod vectorize_test;

/// Hershey vector-font strokes for Latin text — `markdown.rs`'s text
/// pipeline for the one script the raster-trace approach turned out not
/// to suit. See its module doc for what was tried and why.
pub mod hershey;

/// Markdown, laid out and drawn block by block instead of traced whole —
/// text goes through a real font instead of through the whole-page raster
/// that makes small type illegible. See the module doc for the full case.
pub mod markdown;

#[cfg(test)]
#[path = "markdown_test.rs"]
mod markdown_test;

#[cfg(test)]
#[path = "rmfile_test.rs"]
mod rmfile_test;

#[cfg(test)]
#[path = "upload_test.rs"]
mod upload_test;

#[cfg(test)]
#[path = "screen_test.rs"]
mod screen_test;
