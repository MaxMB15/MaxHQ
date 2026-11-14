// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Small previews of the pictures you attach, made as `maxd` stages them,
//! so a client can show a picture without reading the file. A preview is a
//! `data:` URL that holds a JPEG, or a PNG when the picture has see-through
//! parts, with its long edge at most [`LONG_EDGE`] pixels. A file that
//! isn't a picture, won't decode, is too large to read or to decode, or
//! would make too long a preview gets none, and the client shows its
//! placeholder.

use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, ImageResult, Limits};

/// The longest edge a preview has, in pixels. A larger picture shrinks to
/// it, keeping its shape.
const LONG_EDGE: u32 = 320;

/// How large a picture's file can be and still be read for a preview.
const READ_CAP: u64 = 32 * 1024 * 1024;

/// How much memory decoding one picture can take, with the copies made as
/// it shrinks. A 48-megapixel photo's pixels take 146 MB of it.
const DECODE_CAP: u64 = 256 * 1024 * 1024;

/// The longest side a picture can have and still be decoded, in pixels,
/// which leaves room for a phone's panorama.
const SIDE_CAP: u32 = 32 * 1024;

/// The most a pixel takes of any format decoded here: a 16-bit PNG with
/// alpha.
const MOST_PER_PIXEL: u64 = 8;

/// How long a preview's `data:` URL can be.
const URL_CAP: usize = 48 * 1024;

/// The quality a JPEG preview is saved at, out of 100.
const JPEG_QUALITY: u8 = 80;

/// Whether `mime` names a picture a preview can be made of.
fn is_picture(mime: &str) -> bool {
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    )
}

/// A preview of the picture at `path` as a `data:` URL, or none when there
/// isn't one to make. `mime` is the file's media type, which says whether
/// it's a picture at all.
pub fn thumbnail(path: &Path, mime: Option<&str>) -> Option<String> {
    if !is_picture(mime?) {
        return None;
    }
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > READ_CAP {
        return None;
    }
    let bytes = read_within(fs::File::open(path).ok()?, READ_CAP)?;
    thumbnail_of(&bytes, mime)
}

/// Everything in `file` when that's `cap` bytes or fewer, and none otherwise.
/// A file can grow after its size is checked, so the read has a cap of its own.
fn read_within(file: impl Read, cap: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= cap).then_some(bytes)
}

/// A preview of the picture in `bytes`, as [`thumbnail`] makes for a file.
pub fn thumbnail_of(bytes: &[u8], mime: Option<&str>) -> Option<String> {
    if !is_picture(mime?) || bytes.len() as u64 > READ_CAP {
        return None;
    }
    let (media, data) = match preview(bytes) {
        Ok(preview) => preview,
        Err(error) => {
            tracing::debug!(%error, "no preview, since the picture couldn't be read");
            return None;
        }
    };
    let url = format!("data:{media};base64,{}", STANDARD.encode(data));
    (url.len() <= URL_CAP).then_some(url)
}

/// Whether `url` is a preview as [`thumbnail`] makes one, for a preview
/// read back from a file.
pub fn is_preview(url: &str) -> bool {
    let data = url
        .strip_prefix("data:image/jpeg;base64,")
        .or_else(|| url.strip_prefix("data:image/png;base64,"));
    url.len() <= URL_CAP
        && data.is_some_and(|data| {
            !data.is_empty()
                && data
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
        })
}

/// The picture in `bytes` as a PNG with its long edge at most `long_edge`
/// pixels, such as a test machine's screenshot for an agent. A PNG that
/// fits already is answered as it is.
pub fn png_within(bytes: &[u8], long_edge: u32) -> ImageResult<Vec<u8>> {
    let (picture, changed) = shrunk(bytes, long_edge)?;
    if !changed && image::guess_format(bytes).is_ok_and(|format| format == ImageFormat::Png) {
        return Ok(bytes.to_vec());
    }
    let mut out = Vec::new();
    picture.write_with_encoder(PngEncoder::new(&mut out))?;
    Ok(out)
}

/// A decoder for the picture in `bytes`, refusing one whose pixels and the
/// decoder's own buffers wouldn't fit in `cap` bytes. The format is guessed
/// from the bytes. `DynamicImage::from_decoder` allocates whatever the
/// header claims without asking the limits, and a header takes a few bytes
/// to write, so what it claims is counted here first.
pub fn open(bytes: &[u8], cap: u64) -> ImageResult<impl ImageDecoder + '_> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(SIDE_CAP);
    limits.max_image_height = Some(SIDE_CAP);
    limits.max_alloc = Some(cap);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder()?;
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    Ok(decoder)
}

/// The picture in `bytes` shrunk to fit `long_edge`, and whether that
/// changed its pixels. The format is guessed from the bytes.
fn shrunk(bytes: &[u8], long_edge: u32) -> ImageResult<(DynamicImage, bool)> {
    // Beside the full picture sit the copy it shrinks to and those made to
    // turn it and to change its color type, each at most `long_edge` on a
    // side.
    let copies = 3 * MOST_PER_PIXEL * u64::from(long_edge).pow(2);
    let mut decoder = open(bytes, DECODE_CAP.saturating_sub(copies))?;
    // A camera's photo stores its rotation apart from its pixels, and the
    // preview keeps no such note, so the rotation goes in the pixels.
    let orientation = decoder.orientation()?;
    let mut picture = DynamicImage::from_decoder(decoder)?;
    let large = picture.width().max(picture.height()) > long_edge;
    if large {
        picture = picture.thumbnail(long_edge, long_edge);
    }
    picture.apply_orientation(orientation);
    Ok((picture, large || orientation != Orientation::NoTransforms))
}

/// The picture in `bytes` shrunk to fit [`LONG_EDGE`], with the media type
/// it's encoded as. The file's own format is guessed from its bytes.
fn preview(bytes: &[u8]) -> ImageResult<(&'static str, Vec<u8>)> {
    let (picture, _) = shrunk(bytes, LONG_EDGE)?;
    let mut out = Vec::new();
    if see_through(&picture) {
        DynamicImage::ImageRgba8(picture.to_rgba8())
            .write_with_encoder(PngEncoder::new(&mut out))?;
        Ok(("image/png", out))
    } else {
        DynamicImage::ImageRgb8(picture.to_rgb8())
            .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))?;
        Ok(("image/jpeg", out))
    }
}

/// Whether any of the picture's pixels can be seen through. A picture can
/// carry an alpha channel and still be opaque throughout, as many
/// screenshots are, and a JPEG of one is far smaller.
fn see_through(picture: &DynamicImage) -> bool {
    picture.color().has_alpha()
        && picture
            .to_rgba8()
            .pixels()
            .any(|pixel| pixel.0[3] < u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use image::{
        GenericImageView, ImageEncoder, ImageError, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage,
    };
    use std::fs::File;
    use std::path::PathBuf;

    #[test]
    fn a_file_longer_than_it_looked_is_read_only_to_the_cap() {
        let mut long = std::io::repeat(7).take(4096);
        assert_eq!(read_within(long.by_ref(), 1024), None);
        assert!(long.limit() >= 4096 - 1025, "{}", long.limit());
        assert_eq!(read_within(&[1u8, 2, 3][..], 3), Some(vec![1, 2, 3]));
        assert_eq!(read_within(&[1u8, 2, 3, 4][..], 3), None);
    }

    /// A GIF of a few bytes whose screen is `width` by `height`, with one
    /// pixel of a frame.
    fn gif_claiming(width: u16, height: u16) -> Vec<u8> {
        let mut out = b"GIF89a".to_vec();
        out.extend(width.to_le_bytes());
        out.extend(height.to_le_bytes());
        out.extend([0, 0, 0, 0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0x80]);
        out.extend([0, 0, 0, 255, 255, 255, 2, 2, 0x44, 1, 0, 0x3b]);
        out
    }

    /// The headers of a color JPEG of `width` by `height`, without pixels.
    fn jpeg_claiming(width: u16, height: u16) -> Vec<u8> {
        let mut out = vec![0xff, 0xd8, 0xff, 0xdb, 0, 67, 0];
        out.extend([1; 64]);
        out.extend([0xff, 0xc0, 0, 17, 8]);
        out.extend(height.to_be_bytes());
        out.extend(width.to_be_bytes());
        out.extend([3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0]);
        out.extend([0xff, 0xc4, 0, 20, 0, 1]);
        out.extend([0; 16]);
        out.extend([0xff, 0xc4, 0, 20, 0x10, 1]);
        out.extend([0; 16]);
        out.extend([0xff, 0xda, 0, 12, 3, 1, 0, 2, 0, 3, 0, 0, 63, 0]);
        out.extend([0; 16]);
        out.extend([0xff, 0xd9]);
        out
    }

    /// The header of a lossless WebP with alpha, without pixels. The format
    /// holds 14 bits a side.
    fn webp_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend(26u32.to_le_bytes());
        out.extend(b"WEBPVP8L");
        out.extend(14u32.to_le_bytes());
        out.push(0x2f);
        out.extend(((width - 1) | (height - 1) << 14 | 1 << 28).to_le_bytes());
        out.extend([0; 9]);
        out
    }

    /// Whether the picture is refused for what its header claims, before
    /// anything is decoded.
    fn refused(bytes: &[u8], cap: u64) -> bool {
        matches!(open(bytes, cap).err(), Some(ImageError::Limits(_)))
    }

    /// A smooth picture of `width` by `height`, saved at `name` in `dir`.
    fn picture(dir: &TempDir, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.path().join(name);
        RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        })
        .save(&path)
        .expect("a picture");
        path
    }

    /// The picture a preview holds, with its media type.
    fn decoded(url: &str) -> (String, DynamicImage) {
        let (head, data) = url
            .strip_prefix("data:")
            .and_then(|rest| rest.split_once(";base64,"))
            .expect("a data URL");
        let bytes = STANDARD.decode(data).expect("base64");
        let format = ImageFormat::from_mime_type(head).expect("a picture type");
        let picture = image::load_from_memory_with_format(&bytes, format).expect("a picture");
        (head.to_owned(), picture)
    }

    #[test]
    fn a_large_picture_shrinks_to_a_jpeg_of_the_same_shape() {
        let dir = TempDir::new();
        let path = picture(&dir, "wide.png", 1280, 640);

        let url = thumbnail(&path, Some("image/png")).expect("a preview");

        let (media, preview) = decoded(&url);
        assert_eq!(media, "image/jpeg");
        assert_eq!(preview.dimensions(), (LONG_EDGE, LONG_EDGE / 2));
    }

    #[test]
    fn a_tall_picture_shrinks_by_its_height() {
        let dir = TempDir::new();
        let path = picture(&dir, "tall.jpg", 300, 960);

        let url = thumbnail(&path, Some("image/jpeg")).expect("a preview");

        assert_eq!(decoded(&url).1.dimensions(), (100, LONG_EDGE));
    }

    #[test]
    fn a_small_picture_keeps_its_size() {
        let dir = TempDir::new();
        let path = picture(&dir, "icon.gif", 40, 30);

        let url = thumbnail(&path, Some("image/gif")).expect("a preview");

        assert_eq!(decoded(&url).1.dimensions(), (40, 30));
    }

    #[test]
    fn a_photo_turns_the_way_the_camera_held_it() {
        let dir = TempDir::new();
        let path = dir.path().join("photo.jpg");
        // The smallest EXIF that says to turn the picture a quarter right.
        let exif = [
            b'I', b'I', 42, 0, 8, 0, 0, 0, // the header, then where the tags start
            1, 0, // one tag
            0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, // orientation 6
            0, 0, 0, 0, // no more tags
        ];
        let mut out = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut out, 90);
        encoder.set_exif_metadata(exif.to_vec()).expect("EXIF");
        DynamicImage::ImageRgb8(RgbImage::from_pixel(640, 320, Rgb([90, 120, 150])))
            .write_with_encoder(encoder)
            .expect("a photo");
        fs::write(&path, out).expect("a file");

        let url = thumbnail(&path, Some("image/jpeg")).expect("a preview");

        assert_eq!(decoded(&url).1.dimensions(), (160, LONG_EDGE));
    }

    #[test]
    fn a_picture_with_see_through_parts_stays_a_png() {
        let dir = TempDir::new();
        let path = dir.path().join("cutout.png");
        RgbaImage::from_fn(640, 480, |x, _| {
            Rgba([200, 40, 40, if x < 320 { 0 } else { 255 }])
        })
        .save(&path)
        .expect("a picture");

        let url = thumbnail(&path, Some("image/png")).expect("a preview");

        let (media, preview) = decoded(&url);
        assert_eq!(media, "image/png");
        assert_eq!(preview.dimensions(), (LONG_EDGE, 240));
        assert_eq!(preview.get_pixel(0, 0).0[3], 0);
        assert_eq!(preview.get_pixel(LONG_EDGE - 1, 0).0[3], 255);
    }

    #[test]
    fn an_opaque_picture_with_an_alpha_channel_goes_as_a_jpeg() {
        let dir = TempDir::new();
        let path = dir.path().join("screenshot.png");
        RgbaImage::from_pixel(800, 500, Rgba([30, 30, 30, 255]))
            .save(&path)
            .expect("a picture");

        let url = thumbnail(&path, Some("image/png")).expect("a preview");

        assert_eq!(decoded(&url).0, "image/jpeg");
    }

    #[test]
    fn the_media_type_need_not_match_the_bytes() {
        let dir = TempDir::new();
        let path = picture(&dir, "photo.png", 64, 64);

        assert!(thumbnail(&path, Some("image/webp")).is_some());
    }

    #[test]
    fn knows_a_preview_read_back() {
        let dir = TempDir::new();
        let path = picture(&dir, "small.png", 40, 30);
        assert!(is_preview(&thumbnail(&path, Some("image/png")).unwrap()));
        for bad in [
            "",
            "data:image/png;base64,",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:text/html;base64,PGI+",
            "data:image/png;base64,ab\"cd",
            "https://example.com/a.png",
        ] {
            assert!(!is_preview(bad), "{bad}");
        }
        let long = format!("data:image/png;base64,{}", "A".repeat(URL_CAP));
        assert!(!is_preview(&long));
    }

    #[test]
    fn a_header_claiming_too_much_is_refused_before_decoding() {
        // Each takes a few bytes or a few kilobytes on disk.
        assert!(refused(&crate::testing::HUGE_PNG, DECODE_CAP));
        assert!(refused(&gif_claiming(u16::MAX, u16::MAX), DECODE_CAP));
        assert!(refused(&gif_claiming(20_000, 20_000), DECODE_CAP));
        assert!(refused(&jpeg_claiming(12_000, 12_000), DECODE_CAP));
        assert!(refused(&webp_claiming(12_000, 12_000), DECODE_CAP));
        // The longest side counts however little the picture holds.
        assert!(refused(&jpeg_claiming(40_000, 1), DECODE_CAP));
    }

    #[test]
    fn a_large_phone_photo_is_within_the_cap() {
        // 48 megapixels, and 50 and 64 as other phones save them.
        assert!(open(&jpeg_claiming(8064, 6048), DECODE_CAP).is_ok());
        assert!(open(&jpeg_claiming(8160, 6120), DECODE_CAP).is_ok());
        assert!(open(&jpeg_claiming(9248, 6936), DECODE_CAP).is_ok());
        // A long panorama from a phone.
        assert!(open(&jpeg_claiming(30_000, 2000), DECODE_CAP).is_ok());
        // The biggest honest pictures on a screen, a GIF being four bytes a
        // pixel.
        assert!(open(&gif_claiming(7680, 4320), DECODE_CAP).is_ok());
        assert!(open(&webp_claiming(7680, 4320), DECODE_CAP).is_ok());
    }

    #[test]
    fn no_preview_for_a_picture_claiming_too_much() {
        if crate::testing::apart("thumbnail::tests::no_preview_for_a_picture_claiming_too_much") {
            return;
        }
        let dir = TempDir::new();
        let huge = dir.path().join("huge.png");
        fs::write(&huge, crate::testing::HUGE_PNG).unwrap();
        assert_eq!(thumbnail(&huge, Some("image/png")), None);
        let bytes = gif_claiming(u16::MAX, u16::MAX);
        assert_eq!(thumbnail_of(&bytes, Some("image/gif")), None);
        assert!(png_within(&crate::testing::HUGE_PNG, 1280).is_err());
    }

    #[test]
    fn no_preview_for_what_isnt_a_picture() {
        let dir = TempDir::new();
        let png = picture(&dir, "shot.png", 64, 64);
        let notes = dir.path().join("notes.png");
        fs::write(&notes, "not a picture").expect("a file");

        assert_eq!(thumbnail(&png, None), None);
        assert_eq!(thumbnail(&png, Some("application/pdf")), None);
        assert_eq!(thumbnail(&png, Some("image/tiff")), None);
        assert_eq!(thumbnail(&notes, Some("image/png")), None);
        assert_eq!(thumbnail(dir.path(), Some("image/png")), None);
        assert_eq!(
            thumbnail(&dir.path().join("gone.png"), Some("image/png")),
            None
        );
    }

    #[test]
    fn no_preview_for_a_file_too_large_to_read() {
        let dir = TempDir::new();
        let path = picture(&dir, "huge.png", 64, 64);
        assert!(thumbnail(&path, Some("image/png")).is_some());

        // A decoder stops at the picture's end, so only the size differs.
        File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_len(READ_CAP + 1))
            .expect("a larger file");

        assert_eq!(thumbnail(&path, Some("image/png")), None);
    }

    #[test]
    fn no_preview_when_it_would_be_too_long() {
        let dir = TempDir::new();
        let path = dir.path().join("noise.png");
        let mut seed: u32 = 0x9e37_79b9;
        RgbImage::from_fn(LONG_EDGE, LONG_EDGE, |_, _| {
            let mut channel = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed.to_le_bytes()[0]
            };
            Rgb([channel(), channel(), channel()])
        })
        .save(&path)
        .expect("a picture");

        assert_eq!(thumbnail(&path, Some("image/png")), None);
    }

    #[test]
    fn a_png_shrinks_to_fit_and_a_small_one_stays_as_it_is() {
        let dir = TempDir::new();
        let large = std::fs::read(picture(&dir, "large.png", 2000, 1000)).expect("bytes");
        let small = std::fs::read(picture(&dir, "small.png", 300, 200)).expect("bytes");
        let photo = std::fs::read(picture(&dir, "small.jpg", 300, 200)).expect("bytes");

        let shrunk = png_within(&large, 1280).expect("a PNG");
        let shrunk = image::load_from_memory_with_format(&shrunk, ImageFormat::Png).expect("a PNG");
        assert_eq!(shrunk.dimensions(), (1280, 640));
        assert_eq!(png_within(&small, 1280).expect("a PNG"), small);
        let photo = png_within(&photo, 1280).expect("a PNG");
        assert_eq!(
            image::guess_format(&photo).expect("a format"),
            ImageFormat::Png
        );
        assert!(png_within(b"not a picture", 1280).is_err());
    }
}
