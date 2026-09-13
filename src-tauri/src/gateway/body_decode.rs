//! Request-body decompression for incoming gateway requests.
//!
//! Codex.app (and other "production-grade" HTTP clients that treat the
//! gateway as a real OpenAI endpoint, e.g. when `requires_openai_auth =
//! true`) compresses request bodies with gzip or deflate by default. The
//! axum `String` extractor would then explode with
//! `Request body didn't contain valid UTF-8: invalid utf-8 sequence of 1
//! bytes from index 1` — the gzip magic header (`1f 8b ...`) failing UTF-8
//! decoding.
//!
//! This module replaces the `String` extractor with a `Bytes`-then-decode
//! pattern: handlers take `body: Bytes`, then call `decode(headers, body)`
//! to get a `String` honouring whatever `Content-Encoding` the client
//! advertised.

use crate::errors::AppError;
use axum::http::HeaderMap;
use bytes::Bytes;
use std::io::Read;

/// Decode the request body, honouring `Content-Encoding`. Returns the
/// decoded UTF-8 string ready for JSON parsing. Identity / absent encoding
/// is the common case and short-circuits to a single allocation.
///
/// Supported encodings: gzip / x-gzip / deflate / br / zstd / identity.
/// Modern clients (Codex.app, ChatGPT desktop) default to zstd; older /
/// generic ones use gzip or deflate. Brotli covers some browser-style
/// stacks that may show up via embedded webviews.
///
/// `limit` 是网关配置的请求体上限(字节)。`DefaultBodyLimit` 只限制压缩后的
/// 大小,解压必须再按同一上限封顶,否则几十 KB 的 gzip / zstd / brotli 炸弹
/// 就能把网关内存打爆。
pub fn decode(headers: &HeaderMap, body: Bytes, limit: usize) -> Result<String, AppError> {
    let encoding = headers
        .get(axum::http::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    // Strip optional " ; q=..." weights some clients append.
    let primary = encoding
        .split(',')
        .next()
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let decoded: Vec<u8> = match primary.as_str() {
        "" | "identity" => body.to_vec(),
        "gzip" | "x-gzip" => decompress_gzip(&body, limit)?,
        "deflate" => decompress_deflate(&body, limit)?,
        "br" => decompress_brotli(&body, limit)?,
        "zstd" => decompress_zstd(&body, limit)?,
        // Some clients chain encodings ("gzip, br" etc.). Try each in
        // order — almost always single-encoding in practice.
        multi if multi.contains("gzip") => decompress_gzip(&body, limit)?,
        multi if multi.contains("deflate") => decompress_deflate(&body, limit)?,
        multi if multi.contains("br") => decompress_brotli(&body, limit)?,
        multi if multi.contains("zstd") => decompress_zstd(&body, limit)?,
        other => {
            return Err(AppError::new(
                crate::errors::codes::UNSUPPORTED_CONTENT_ENCODING,
                format!("Request body uses unsupported Content-Encoding: {other}"),
            ));
        }
    };

    String::from_utf8(decoded).map_err(|e| {
        AppError::new(
            crate::errors::codes::INVALID_REQUEST_BODY,
            format!("Decoded body is not valid UTF-8: {e}"),
        )
    })
}

/// 从解压 reader 里最多读 `limit + 1` 字节:读满说明解压后超限,按请求体过大拒绝。
fn read_bounded(
    reader: impl Read,
    compressed_len: usize,
    limit: usize,
    decode_error: impl FnOnce(std::io::Error) -> AppError,
) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::with_capacity(compressed_len.saturating_mul(2).min(limit));
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(decode_error)?;
    if out.len() > limit {
        return Err(AppError::new(
            crate::errors::codes::REQUEST_BODY_TOO_LARGE,
            "请求内容解压后超过网关请求体上限",
        )
        .with_detail(format!(
            "压缩后 {compressed_len} 字节,解压后超过 {limit} 字节上限"
        ))
        .with_suggestion("请新开会话或减少本次发送内容；也可在设置里调大请求体上限，或设置 AGENTGATE_REQUEST_BODY_LIMIT_MB 后重启网关"));
    }
    Ok(out)
}

fn decompress_gzip(data: &[u8], limit: usize) -> Result<Vec<u8>, AppError> {
    read_bounded(flate2::read::GzDecoder::new(data), data.len(), limit, |e| {
        AppError::new(
            crate::errors::codes::GZIP_DECODE_FAILED,
            format!("Failed to decompress gzip body: {e}"),
        )
    })
}

fn decompress_deflate(data: &[u8], limit: usize) -> Result<Vec<u8>, AppError> {
    read_bounded(
        flate2::read::DeflateDecoder::new(data),
        data.len(),
        limit,
        |e| {
            AppError::new(
                crate::errors::codes::DEFLATE_DECODE_FAILED,
                format!("Failed to decompress deflate body: {e}"),
            )
        },
    )
}

fn decompress_brotli(data: &[u8], limit: usize) -> Result<Vec<u8>, AppError> {
    read_bounded(
        brotli::Decompressor::new(data, 4096),
        data.len(),
        limit,
        |e| {
            AppError::new(
                crate::errors::codes::BROTLI_DECODE_FAILED,
                format!("Failed to decompress brotli body: {e}"),
            )
        },
    )
}

fn decompress_zstd(data: &[u8], limit: usize) -> Result<Vec<u8>, AppError> {
    let decoder = zstd::stream::Decoder::new(data).map_err(|e| {
        AppError::new(
            crate::errors::codes::ZSTD_DECODE_FAILED,
            format!("Failed to init zstd decoder: {e}"),
        )
    })?;
    read_bounded(decoder, data.len(), limit, |e| {
        AppError::new(
            crate::errors::codes::ZSTD_DECODE_FAILED,
            format!("Failed to decompress zstd body: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};
    use flate2::write::{DeflateEncoder, GzEncoder};
    use flate2::Compression;
    use std::io::Write;

    const TEST_LIMIT: usize = 32 * 1024 * 1024;

    #[test]
    fn decompression_bombs_are_rejected_at_the_request_body_limit() {
        // 2MB 的 0 压缩后只有几 KB,远小于 DefaultBodyLimit;解压必须按同一上限封顶。
        let payload = vec![b'0'; 2 * 1024 * 1024];
        let limit = 1024 * 1024;

        let mut gz = GzEncoder::new(Vec::new(), Compression::best());
        gz.write_all(&payload).unwrap();
        let gz = gz.finish().unwrap();
        let mut deflate = DeflateEncoder::new(Vec::new(), Compression::best());
        deflate.write_all(&payload).unwrap();
        let deflate = deflate.finish().unwrap();
        let zstd_body = zstd::stream::encode_all(&payload[..], 19).unwrap();
        let mut br = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut br, 4096, 9, 22);
            writer.write_all(&payload).unwrap();
        }

        for (enc, body) in [
            ("gzip", gz),
            ("deflate", deflate),
            ("zstd", zstd_body),
            ("br", br),
        ] {
            assert!(
                body.len() < limit,
                "{enc} fixture must be small on the wire"
            );
            let err = decode(&hdrs(Some(enc)), Bytes::from(body.clone()), limit).unwrap_err();
            assert_eq!(err.code, "REQUEST_BODY_TOO_LARGE", "{enc}");
            // 上限足够时照常解压
            let ok = decode(&hdrs(Some(enc)), Bytes::from(body), payload.len()).unwrap();
            assert_eq!(ok.len(), payload.len(), "{enc}");
        }
    }

    fn hdrs(encoding: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(enc) = encoding {
            h.insert(
                HeaderName::from_static("content-encoding"),
                HeaderValue::from_str(enc).unwrap(),
            );
        }
        h
    }

    #[test]
    fn passes_through_uncompressed_body() {
        let body = Bytes::from_static(b"{\"hello\":\"world\"}");
        let out = decode(&hdrs(None), body, TEST_LIMIT).unwrap();
        assert_eq!(out, r#"{"hello":"world"}"#);
    }

    #[test]
    fn passes_through_identity_encoding() {
        let body = Bytes::from_static(b"plain text");
        let out = decode(&hdrs(Some("identity")), body, TEST_LIMIT).unwrap();
        assert_eq!(out, "plain text");
    }

    #[test]
    fn decompresses_gzip_body() {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(br#"{"model":"gpt-4","messages":[]}"#).unwrap();
        let compressed = Bytes::from(e.finish().unwrap());
        let out = decode(&hdrs(Some("gzip")), compressed, TEST_LIMIT).unwrap();
        assert_eq!(out, r#"{"model":"gpt-4","messages":[]}"#);
    }

    #[test]
    fn decompresses_x_gzip_alias() {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"hello").unwrap();
        let compressed = Bytes::from(e.finish().unwrap());
        let out = decode(&hdrs(Some("x-gzip")), compressed, TEST_LIMIT).unwrap();
        assert_eq!(out, "hello");
    }

    #[test]
    fn decompresses_deflate_body() {
        let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"deflate payload").unwrap();
        let compressed = Bytes::from(e.finish().unwrap());
        let out = decode(&hdrs(Some("deflate")), compressed, TEST_LIMIT).unwrap();
        assert_eq!(out, "deflate payload");
    }

    #[test]
    fn case_insensitive_encoding_header() {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"x").unwrap();
        let compressed = Bytes::from(e.finish().unwrap());
        let out = decode(&hdrs(Some("GZIP")), compressed, TEST_LIMIT).unwrap();
        assert_eq!(out, "x");
    }

    #[test]
    fn picks_first_recognised_in_chained_encoding() {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(b"chained").unwrap();
        let compressed = Bytes::from(e.finish().unwrap());
        let out = decode(&hdrs(Some("gzip, br")), compressed, TEST_LIMIT).unwrap();
        assert_eq!(out, "chained");
    }

    #[test]
    fn decompresses_zstd_body() {
        let payload = br#"{"model":"gpt-4","input":"hi"}"#;
        let compressed = Bytes::from(zstd::stream::encode_all(&payload[..], 3).unwrap());
        let out = decode(&hdrs(Some("zstd")), compressed, TEST_LIMIT).unwrap();
        assert_eq!(out, r#"{"model":"gpt-4","input":"hi"}"#);
    }

    #[test]
    fn decompresses_brotli_body() {
        let payload = b"brotli payload";
        let mut compressed = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut compressed, 4096, 4, 22);
            writer.write_all(payload).unwrap();
        }
        let out = decode(&hdrs(Some("br")), Bytes::from(compressed), TEST_LIMIT).unwrap();
        assert_eq!(out, "brotli payload");
    }

    #[test]
    fn rejects_unknown_encoding() {
        let body = Bytes::from_static(b"x");
        let err = decode(&hdrs(Some("xpress")), body, TEST_LIMIT).unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED_CONTENT_ENCODING");
    }

    #[test]
    fn strips_quality_factors_from_encoding_header() {
        // Some clients append "; q=1.0" quality factors.
        let payload = br#"{"x":1}"#;
        let compressed = Bytes::from(zstd::stream::encode_all(&payload[..], 3).unwrap());
        let out = decode(&hdrs(Some("zstd; q=1.0")), compressed, TEST_LIMIT).unwrap();
        assert_eq!(out, r#"{"x":1}"#);
    }

    #[test]
    fn surfaces_gzip_decoder_error_on_bad_data() {
        // Random non-gzip bytes labelled as gzip → decoder error, not panic.
        let body = Bytes::from_static(b"not gzip");
        let err = decode(&hdrs(Some("gzip")), body, TEST_LIMIT).unwrap_err();
        assert_eq!(err.code, "GZIP_DECODE_FAILED");
    }

    #[test]
    fn surfaces_utf8_error_after_successful_decompression() {
        // gzipped non-UTF-8 bytes → decompresses, then fails the UTF-8 step.
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(&[0xff, 0xfe, 0xfd]).unwrap();
        let compressed = Bytes::from(e.finish().unwrap());
        let err = decode(&hdrs(Some("gzip")), compressed, TEST_LIMIT).unwrap_err();
        assert_eq!(err.code, "INVALID_REQUEST_BODY");
    }
}
