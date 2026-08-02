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
