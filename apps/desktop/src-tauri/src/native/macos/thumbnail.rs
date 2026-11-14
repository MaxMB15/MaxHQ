// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Thumbnails from Quick Look, the same previews Finder shows. A file
//! without one gets its icon instead.

// Quick Look's bindings are all marked `unsafe`, and the encoder takes an
// untyped dictionary.
#![allow(unsafe_code)]

use std::path::Path;
use std::ptr;
use std::sync::{Mutex, PoisonError};

use block2::RcBlock;
use objc2::rc::autoreleasepool;
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGImage,
    CGImageAlphaInfo, CGInterpolationQuality,
};
use objc2_foundation::{NSDictionary, NSError, NSURL};
use objc2_quick_look_thumbnailing::{
    QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
    QLThumbnailGenerator, QLThumbnailRepresentation,
};
use tokio::sync::oneshot;

/// Asks Quick Look for a thumbnail at most `size` points on a side, at
/// `scale` pixels per point. The answer is PNG bytes, or none when Quick
/// Look has nothing. Returns none at once for a file that's gone, which
/// Quick Look would still give a generic icon, and for a path no URL can
/// hold.
pub fn request(path: &Path, size: f64, scale: f64) -> Option<oneshot::Receiver<Option<Vec<u8>>>> {
    if !path.exists() {
        return None;
    }
    let url = NSURL::from_file_path(path)?;
    // SAFETY: A file URL, a positive size and scale, and representation
    // flags from the framework.
    let request = unsafe {
        QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
            QLThumbnailGenerationRequest::alloc(),
            &url,
            CGSize::new(size, size),
            scale,
            QLThumbnailGenerationRequestRepresentationTypes::All,
        )
    };
    // Icons come in fixed sizes, and Quick Look can pick one a step
    // larger than asked, so the encoder shrinks those to fit.
    let largest = (size * scale).ceil() as usize;
    let (send, answer) = oneshot::channel();
    // Quick Look calls back on a queue of its own.
    let send = Mutex::new(Some(send));
    let done = RcBlock::new(
        move |thumbnail: *mut QLThumbnailRepresentation, _error: *mut NSError| {
            // SAFETY: Quick Look passes a valid representation or null.
            let png =
                unsafe { thumbnail.as_ref() }.and_then(|thumbnail| encode(thumbnail, largest));
            if let Some(send) = send.lock().unwrap_or_else(PoisonError::into_inner).take() {
                let _ = send.send(png);
            }
        },
    );
    // SAFETY: The generator copies the block and calls it once.
    unsafe {
        QLThumbnailGenerator::sharedGenerator()
            .generateBestRepresentationForRequest_completionHandler(&request, &done);
    }
    Some(answer)
}

/// A thumbnail as PNG bytes, at most `largest` pixels on a side.
fn encode(thumbnail: &QLThumbnailRepresentation, largest: usize) -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        // SAFETY: Every representation carries an image.
        let image = unsafe { thumbnail.CGImage() };
        let shrunk = shrink(&image, largest);
        let image = shrunk.as_deref().unwrap_or(&image);
        let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
        // SAFETY: An empty dictionary asks for no encoder properties.
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }?;
        Some(png.to_vec())
    })
}

/// The image scaled down to at most `largest` pixels on its longer side,
/// keeping its shape, or none when it already fits.
fn shrink(image: &CGImage, largest: usize) -> Option<CFRetained<CGImage>> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    let longer = width.max(height);
    if longer <= largest || largest == 0 {
        return None;
    }
    let fit = |side: usize| (side * largest).div_ceil(longer).max(1);
    let (width, height) = (fit(width), fit(height));
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: A null data pointer and a zero row length let Core Graphics
    // allocate the pixels, eight bits a channel with alpha last.
    let context = unsafe {
        CGBitmapContextCreate(
            ptr::null_mut(),
            width,
            height,
            8,
            0,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
    let bounds = CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64));
    CGContext::draw_image(Some(&context), bounds, Some(image));
    CGBitmapContextCreateImage(Some(&context))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// Reads a PNG's width and height from its header.
    fn dimensions(png: &[u8]) -> (u32, u32) {
        let number = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        (number(16), number(20))
    }

    #[tokio::test]
    #[ignore = "asks Quick Look for thumbnails"]
    async fn thumbnails_files_and_folders_within_the_size() {
        let dir = std::env::temp_dir().join(format!("maxhq-thumbnail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("notes.txt");
        std::fs::write(&text, "Some notes to preview.\n".repeat(40)).unwrap();

        for path in [text.as_path(), dir.as_path()] {
            let started = Instant::now();
            let png = request(path, 64.0, 2.0)
                .unwrap()
                .await
                .unwrap()
                .expect("a thumbnail");
            let (width, height) = dimensions(&png);
            eprintln!(
                "{} {width}x{height} {} bytes in {:?}",
                path.display(),
                png.len(),
                started.elapsed()
            );
            assert!(png.starts_with(b"\x89PNG"));
            assert!(width <= 128 && height <= 128, "{width}x{height}");
        }

        let gone = dir.join("gone.png");
        assert!(request(&gone, 64.0, 2.0).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
