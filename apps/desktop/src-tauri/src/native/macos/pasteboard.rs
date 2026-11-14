// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Files and images on the clipboard, read straight from `NSPasteboard`.
//! The webview's paste event names copied files without their paths, and
//! Tauri's clipboard plugin hands over images as raw pixels.

// Reading objects of a class and encoding a bitmap take untyped arguments.
#![allow(unsafe_code)]

use std::path::PathBuf;

use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2::ClassType;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
    NSPasteboardTypeTIFF, NSPasteboardURLReadingFileURLsOnlyKey,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSURL};

/// The files on the general clipboard.
pub fn clipboard_files() -> Vec<PathBuf> {
    autoreleasepool(|_| file_paths(&NSPasteboard::generalPasteboard()))
}

/// The image on the general clipboard as PNG bytes, unless it holds files.
pub fn clipboard_image() -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        if file_paths(&pasteboard).is_empty() {
            image_png(&pasteboard)
        } else {
            None
        }
    })
}

/// The file paths on a pasteboard.
fn file_paths(pasteboard: &NSPasteboard) -> Vec<PathBuf> {
    let classes = NSArray::from_slice(&[NSURL::class()]);
    let yes = NSNumber::new_bool(true);
    // SAFETY: An AppKit option key constant.
    let file_urls_only = unsafe { NSPasteboardURLReadingFileURLsOnlyKey };
    let options =
        NSDictionary::from_slices(&[file_urls_only], &[AsRef::<AnyObject>::as_ref(&*yes)]);
    // SAFETY: The classes are pasteboard readable, and the option takes a
    // boolean number.
    let objects = unsafe { pasteboard.readObjectsForClasses_options(&classes, Some(&options)) };
    objects
        .iter()
        .flat_map(|objects| objects.iter())
        .filter_map(|object| object.downcast::<NSURL>().ok())
        .filter_map(|url| url.to_file_path())
        .collect()
}

/// A pasteboard's image as PNG bytes. Screenshots arrive as PNG already,
/// and images copied from most apps arrive as TIFF.
fn image_png(pasteboard: &NSPasteboard) -> Option<Vec<u8>> {
    // SAFETY: Both are AppKit's own type constants.
    let (png, tiff) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
    if let Some(data) = pasteboard.dataForType(png) {
        return Some(data.to_vec());
    }
    let tiff = pasteboard.dataForType(tiff)?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
    // SAFETY: An empty dictionary asks for no encoder properties.
    let encoded = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(encoded.to_vec())
}

#[cfg(test)]
mod tests {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2_app_kit::NSPasteboardTypeFileURL;
    use objc2_foundation::{NSData, NSString};

    use super::*;

    /// A private pasteboard that doesn't touch your clipboard, released
    /// when dropped.
    struct Scratch(Retained<NSPasteboard>);

    impl Scratch {
        fn new() -> Self {
            Self(NSPasteboard::pasteboardWithUniqueName())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // SAFETY: `releaseGlobally` takes no arguments and returns
            // nothing.
            let () = unsafe { msg_send![&*self.0, releaseGlobally] };
        }
    }

    /// A tiny PNG, one red pixel.
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    #[ignore = "talks to the macOS pasteboard server"]
    fn reads_file_urls() {
        let scratch = Scratch::new();
        let url = NSURL::from_file_path("/tmp/a b.txt").unwrap();
        scratch.0.clearContents();
        // SAFETY: An AppKit type constant.
        let file_url = unsafe { NSPasteboardTypeFileURL };
        assert!(scratch
            .0
            .setString_forType(&url.absoluteString().unwrap(), file_url));
        assert_eq!(file_paths(&scratch.0), [PathBuf::from("/tmp/a b.txt")]);
    }

    #[test]
    #[ignore = "talks to the macOS pasteboard server"]
    fn reads_png_and_turns_tiff_into_png() {
        let scratch = Scratch::new();
        // SAFETY: AppKit type constants.
        let (png, tiff) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };

        scratch.0.clearContents();
        assert!(scratch
            .0
            .setData_forType(Some(&NSData::with_bytes(PIXEL)), png));
        assert_eq!(image_png(&scratch.0).as_deref(), Some(PIXEL));

        let bitmap = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(PIXEL)).unwrap();
        let tiff_data = bitmap.TIFFRepresentation().unwrap();
        scratch.0.clearContents();
        assert!(scratch.0.setData_forType(Some(&tiff_data), tiff));
        let converted = image_png(&scratch.0).expect("the TIFF converts");
        assert!(converted.starts_with(b"\x89PNG"));

        scratch.0.clearContents();
        assert!(scratch
            .0
            .setString_forType(&NSString::from_str("words"), unsafe {
                objc2_app_kit::NSPasteboardTypeString
            }));
        assert_eq!(image_png(&scratch.0), None);
        assert!(file_paths(&scratch.0).is_empty());
    }
}
