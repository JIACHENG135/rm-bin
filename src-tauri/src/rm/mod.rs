//! Drawing an image onto a real reMarkable, by wrapping it as a one-page PDF
//! and handing it to the tablet's own importer.

pub mod device;

/// Putting files into xochitl's document store over ssh — the PDF
/// importer's fallback path.
pub mod upload;

/// `.metadata` JSON and uuid formatting that `pdf.rs`'s ssh fallback needs.
pub mod rmfile;

/// The image as a one-page PDF, posted to the tablet's own importer.
pub mod pdf;

/// Asking Gemini what a screenshot is, so the ssh fallback can name and file
/// it instead of carrying the screenshot's own filename into the library.
pub mod gemini;

#[cfg(test)]
#[path = "pdf_test.rs"]
mod pdf_test;

#[cfg(test)]
#[path = "upload_test.rs"]
mod upload_test;
