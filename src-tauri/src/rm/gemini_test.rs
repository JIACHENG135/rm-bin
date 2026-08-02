//! The response side of the Gemini call, without making one.
//!
//! Two things here are worth pinning. The base64 pair is hand-written, so it
//! gets the round trip and the padding cases a library would have brought
//! with it. And `extract_image` searches rather than indexes, on purpose —
//! these tests are what say that the search finds the image in shapes we have
//! not seen, and that an API error comes back as the API's own words instead
//! of "no image found".

use super::gemini::{base64_decode, base64_encode, extension_of, extract_image};

/// The redraw is written to a temp file that the tracer then opens with
/// `image::open`, which picks its decoder from the *file name*. So the name
/// has to follow the bytes. It didn't at first — everything was written
/// `.png`, the model replied with a JPEG, and a perfectly good picture came
/// back as "Invalid PNG signature".
#[test]
fn the_extension_follows_the_bytes_not_the_response() {
    assert_eq!(extension_of(b"\x89PNG\r\n\x1a\n....").unwrap(), "png");
    assert_eq!(extension_of(b"\xff\xd8\xff\xe0JFIF").unwrap(), "jpg");
    assert_eq!(extension_of(b"RIFF\x00\x00\x00\x00WEBPVP8 ").unwrap(), "webp");
    assert_eq!(extension_of(b"GIF89a.......").unwrap(), "gif");
    assert_eq!(extension_of(b"BM........").unwrap(), "bmp");

    // RIFF alone is not WebP, and must not be claimed as one.
    assert!(extension_of(b"RIFF\x00\x00\x00\x00WAVEfmt ").is_err());

    // An unknown blob should say what arrived rather than guess: by this
    // point the request succeeded, so "what is this" is the whole question.
    let err = extension_of(b"\x00\x01\x02\x03junk").unwrap_err();
    assert!(err.contains("00 01 02 03"), "{err}");
    assert!(extension_of(b"").is_err());
}

/// The size floor the prompt tells the model about is a consequence of the
/// tracer's work raster, so it is interpolated from `draw::BASE_WORK` rather
/// than typed twice. If that ever drifts, the prompt starts quietly lying
/// about the machine it is describing.
#[test]
fn the_prompt_describes_the_actual_pipeline() {
    let p = super::gemini::prompt(crate::rm::draw::BASE_WORK as u32);
    assert!(p.contains("700"), "the work raster must be stated: {p}");
    for must in ["Otsu", "Zhang-Suen", "branch", "outlines only", "print letters"] {
        assert!(p.contains(must), "prompt is missing {must:?}");
    }
    // Different raster, different claim — proves it is interpolated.
    assert!(super::gemini::prompt(512).contains("512"));
}

#[test]
fn base64_round_trips_including_the_padding_cases() {
    for n in 0..48usize {
        let data: Vec<u8> = (0..n).map(|i| (i * 7 + 13) as u8).collect();
        let encoded = base64_encode(&data);
        assert_eq!(encoded.len() % 4, 0, "len {n} must be padded to a multiple of 4");
        assert_eq!(base64_decode(&encoded).as_deref(), Some(&data[..]), "len {n}");
    }
    // Known vectors, so a symmetric bug in both halves cannot hide.
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
}

/// What comes back is somebody else's encoder: it may wrap lines, and it may
/// use the URL-safe alphabet.
#[test]
fn base64_decoding_tolerates_wrapping_and_the_url_safe_alphabet() {
    let raw: Vec<u8> = (0..=255u8).collect();
    let wrapped = base64_encode(&raw)
        .as_bytes()
        .chunks(60)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(base64_decode(&wrapped).unwrap(), raw);

    let url_safe = base64_encode(&raw).replace('+', "-").replace('/', "_");
    assert_eq!(base64_decode(&url_safe).unwrap(), raw);

    assert!(base64_decode("not base64!!").is_none());
}

fn body_with_image(b64: &str, key: &str) -> String {
    format!(r#"{{"output":[{{"type":"image","{key}":"image/png","data":"{b64}"}}]}}"#)
}

/// The endpoint has already changed shape once, and the SDKs paper over it
/// with an `output_image` convenience that REST does not have. So the finder
/// is checked against several plausible shapes rather than the one currently
/// documented.
#[test]
fn the_image_is_found_wherever_it_is_nested() {
    let png = b"\x89PNG\r\n\x1a\nfake";
    let b64 = base64_encode(png);

    let shapes = [
        body_with_image(&b64, "mime_type"),
        body_with_image(&b64, "mimeType"),
        format!(r#"{{"output_image":{{"mime_type":"image/png","data":"{b64}"}}}}"#),
        format!(
            r#"{{"candidates":[{{"content":{{"parts":[
                 {{"text":"here you go"}},
                 {{"inlineData":{{"mimeType":"image/png","data":"{b64}"}}}}]}}}}]}}"#
        ),
    ];
    for (i, body) in shapes.iter().enumerate() {
        assert_eq!(extract_image(body.as_bytes()).unwrap(), png, "shape {i}");
    }
}

/// Text parts also carry `data`-ish fields; only an image mime type counts.
#[test]
fn a_text_only_reply_is_not_mistaken_for_an_image() {
    let body = br#"{"output":[{"type":"text","mime_type":"text/plain","data":"Zm9v"}]}"#;
    let err = extract_image(body).unwrap_err();
    assert!(err.contains("没有返回图片"), "{err}");
}

/// An API error is a normal JSON body. Reporting "no image found" for a
/// refused key or an exhausted quota would send anyone looking in the wrong
/// place, so the API's own message has to survive.
#[test]
fn an_api_error_is_reported_in_its_own_words() {
    let body = br#"{"error":{"code":429,"message":"Quota exceeded for images","status":"RESOURCE_EXHAUSTED"}}"#;
    let err = extract_image(body).unwrap_err();
    assert!(err.contains("Quota exceeded for images"), "{err}");
}

#[test]
fn a_non_json_response_says_so() {
    let err = extract_image(b"<html>502 Bad Gateway</html>").unwrap_err();
    assert!(err.contains("无法解析"), "{err}");
    assert!(err.contains("502"), "the body should be quoted back: {err}");
}
