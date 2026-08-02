//! The response side of the Gemini call, without making one.
//!
//! Two things here are worth pinning. The base64 pair is hand-written, so it
//! gets the round trip and the padding cases a library would have brought
//! with it. And `extract_image` searches rather than indexes, on purpose —
//! these tests are what say that the search finds the image in shapes we have
//! not seen, and that an API error comes back as the API's own words instead
//! of "no image found".

use super::gemini::{base64_decode, base64_encode, extract_image};

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
