// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The content blocks a message sends in `session/prompt`. The text names
//! every attachment by path, as `maxhq_core::attached` writes them, so the
//! agent can open each with its own tools. When the CLI says it takes
//! images, each image also goes inline as an `image` block, after its
//! label when the message's pills number it. Otherwise images go by path
//! like everything else.

use std::fs;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_core::attached::{self, Attached};
use maxhq_core::context::{self, File, Sent, Shows};
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::timeline::AttachmentKind;
use serde_json::{json, Value};

use crate::drive::Staged;

/// The largest image sent inline. A bigger one goes by path.
const INLINE_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// The blocks for a message's text, with `refs` placed in it, and its
/// attachments, for the agent `to`. `images` says whether the CLI takes
/// image blocks. `here` says whether the files are on this device. A
/// container's are the guest's, and their paths are its word, so nothing
/// here opens them and its CLI reads them by path.
pub fn blocks(
    text: &str,
    refs: &[InlineRef],
    attachments: &[Staged],
    to: AgentId,
    images: bool,
    here: bool,
) -> Vec<Value> {
    let mut files = Vec::new();
    let mut inline = Vec::new();
    for staged in attachments {
        let path = staged.path.to_string_lossy().into_owned();
        let kind = staged.attachment.kind;
        let block = (here && images && kind == AttachmentKind::Image)
            .then(|| image(staged))
            .flatten();
        let sent = if block.is_some() {
            Sent::Inline
        } else {
            Sent::ByPath
        };
        if let Some(block) = block {
            inline.push((files.len(), block));
        }
        files.push(File {
            id: staged.attachment.id,
            attached: Attached {
                kind,
                name: staged.attachment.name.clone(),
                holds: (here && kind == AttachmentKind::Folder)
                    .then(|| attached::folder_names(&staged.path)),
                path,
                text: staged
                    .text
                    .as_ref()
                    .map(|text| text.to_string_lossy().into_owned()),
            },
            sent,
        });
    }
    // An ACP agent has none of MaxHQ's tools, so a visual pill tells it to
    // answer with a fenced page instead.
    let encoded = context::encode_for(text, refs, &files, to, Shows::Fence);
    let mut blocks = vec![text_block(encoded.text)];
    for (index, label) in &encoded.numbered {
        if let Some(at) = inline.iter().position(|(file, _)| file == index) {
            let (_, block) = inline.remove(at);
            blocks.push(text_block(format!("{label}:")));
            blocks.push(block);
        }
    }
    blocks.extend(inline.into_iter().map(|(_, block)| block));
    blocks
}

/// A text block.
fn text_block(text: String) -> Value {
    json!({ "type": "text", "text": text })
}

/// An image block with the picture's bytes, or none when it can't be read
/// or is too big to send inline.
fn image(staged: &Staged) -> Option<Value> {
    let size = fs::metadata(&staged.path).ok()?.len();
    if size > INLINE_IMAGE_BYTES {
        return None;
    }
    let bytes = fs::read(&staged.path).ok()?;
    let mime = staged
        .attachment
        .mime
        .clone()
        .or_else(|| mime_of(&staged.attachment.name).map(str::to_owned))?;
    Some(json!({
        "type": "image",
        "mimeType": mime,
        "data": STANDARD.encode(bytes),
        "uri": format!("file://{}", staged.path.display()),
    }))
}

/// The picture type a file name says.
fn mime_of(name: &str) -> Option<&'static str> {
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use maxhq_protocol::ids::AttachmentId;
    use maxhq_protocol::timeline::AttachmentRef;
    use std::path::PathBuf;

    fn lead() -> AgentId {
        "0199a7c2-0000-7000-8000-000000000001".parse().unwrap()
    }

    fn staged(path: PathBuf, kind: AttachmentKind) -> Staged {
        Staged {
            attachment: AttachmentRef {
                id: AttachmentId::derive(&path.to_string_lossy()),
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                kind,
                bytes: 1,
                mime: None,
                path: path.to_string_lossy().into_owned(),
                thumbnail: None,
            },
            path,
            text: None,
        }
    }

    /// A container's guest can name any path. Its files aren't on this
    /// device, so what's at that path here stays out of the message.
    #[test]
    fn a_containers_paths_are_named_and_never_opened_here() {
        let temp = TempDir::new();
        let picture = temp.path().join("key.png");
        std::fs::write(&picture, b"png").unwrap();
        let folder = temp.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("private.txt"), b"x").unwrap();
        let files = [
            staged(picture, AttachmentKind::Image),
            staged(folder, AttachmentKind::Folder),
        ];

        let there = blocks("Look", &[], &files, lead(), true, false);
        assert_eq!(there.len(), 1, "{there:?}");
        let text = there[0]["text"].as_str().unwrap();
        assert!(text.contains("key.png"), "{text}");
        assert!(!text.contains("private.txt"), "{text}");

        let here = blocks("Look", &[], &files, lead(), true, true);
        assert_eq!(here.len(), 2, "{here:?}");
        assert!(here[0]["text"].as_str().unwrap().contains("private.txt"));
    }

    /// An ACP agent has no show tool, so a visual pill asks it for a
    /// fenced page.
    #[test]
    fn a_visual_pill_asks_for_a_fenced_page() {
        let temp = TempDir::new();
        let path = temp.path().join("visual-usage.html");
        std::fs::write(&path, b"<p>hi</p>").unwrap();
        let page = staged(path, AttachmentKind::File);
        let refs = [InlineRef::Visual {
            attachment: page.attachment.id,
            title: "Usage".to_owned(),
            from: None,
        }];
        let sent = blocks("Fix \u{FFFC}", &refs, &[page], lead(), true, true);
        let text = sent[0]["text"].as_str().unwrap();
        assert!(text.starts_with("Fix [Visual 1: Usage]"), "{text}");
        assert!(text.contains("```html visual title="), "{text}");
        assert!(!text.contains("show_visual"), "{text}");
    }

    /// The CLI hands the text back when it loads the thread, and the pill
    /// reads back from it.
    #[test]
    fn an_output_pill_comes_back_from_what_the_cli_got() {
        let refs = vec![crate::testing::output_pill()];
        let typed = "Why did \u{FFFC} fail?";
        let sent = blocks(typed, &refs, &[], lead(), true, true);
        assert_eq!(sent.len(), 1);
        let text = sent[0]["text"].as_str().unwrap();
        assert!(text.contains("\n````\nFAIL src/cache.test.ts\n"), "{text}");
        let maxhq_protocol::timeline::ItemBody::UserMessage {
            text: back,
            attachments,
            refs: read,
            ..
        } = super::super::history::user_message("cursor", text)
        else {
            panic!("not a user message");
        };
        assert_eq!(back, typed);
        assert!(attachments.is_empty());
        assert_eq!(read, refs);
    }

    #[test]
    fn text_alone_is_one_block() {
        assert_eq!(
            blocks("hello", &[], &[], lead(), true, true),
            vec![json!({ "type": "text", "text": "hello" })]
        );
    }

    #[test]
    fn an_image_goes_inline_only_when_the_cli_takes_images() {
        let temp = TempDir::new();
        let path = temp.path().join("red.png");
        std::fs::write(&path, b"png").unwrap();
        let image = staged(path.clone(), AttachmentKind::Image);
        let with = blocks(
            "Look",
            &[],
            std::slice::from_ref(&image),
            lead(),
            true,
            true,
        );
        assert_eq!(with.len(), 2);
        assert_eq!(with[1]["type"], "image");
        assert_eq!(with[1]["mimeType"], "image/png");
        assert_eq!(with[1]["data"], STANDARD.encode(b"png"));
        let root = temp.path().display();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            with[0]["text"],
            format!("Look\n\n[Attached image \"red.png\" is saved at: {root}{sep}red.png]")
        );

        let without = blocks("Look", &[], &[image], lead(), false, true);
        assert_eq!(without.len(), 1);
        assert_eq!(
            without[0]["text"],
            format!("Look\n\n[Attached image \"red.png\" is saved at: {root}{sep}red.png]")
        );
    }
}
