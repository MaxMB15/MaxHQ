// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pictures in tool results, such as a Read of a PNG or an MCP tool's
//! screenshot. Each comes as base64 in the result, and the transcript or
//! rollout keeps it the same way. Each is kept as a file in the agent's
//! attachments folder, named for its bytes, so the device can serve it at
//! full size and a history read after a restart lands on the same file
//! again.
//!
//! A container's folders are inside it. There the host only names where a
//! picture goes and hands it to the container's guest, which writes it as
//! its own history reads do, so the file is in place before anyone can
//! open it from the timeline.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_protocol::guest::MAX_PICTURE_LEN;
use maxhq_protocol::ids::{AgentId, AttachmentId, WorkspaceId};
use maxhq_protocol::timeline::{AttachmentKind, AttachmentRef, ItemBody};
use maxhq_transfer::receive::sha256_hex;
use serde_json::Value;

use crate::drive::{Place, PlaceHost};
use crate::thumbnail;

/// The folder inside an agent's attachments folder that holds them.
pub const FOLDER: &str = "tool-output";

/// The most pictures one result keeps.
const MAX_PER_RESULT: usize = 16;

/// The largest picture kept, decoded. The APIs take far smaller ones.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// Where a conversation keeps the pictures its tool results hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pictures {
    /// The attachments root, with a folder for each agent.
    pub root: PathBuf,
    /// The container workspace the root is in, or none when the files are
    /// written from here.
    pub guest: Option<WorkspaceId>,
    /// Whether a preview waits to be made off the session loop, as a
    /// running driver's must. A history read is off the loop already.
    pub later: bool,
}

impl Pictures {
    /// Pictures written under `root`, on this device or, for a guest, in
    /// its container.
    pub fn here(root: PathBuf) -> Self {
        Self {
            root,
            guest: None,
            later: false,
        }
    }

    /// Where the agents at `place` keep pictures: beside its attachments
    /// folder, and in its container when it has one.
    pub fn at(place: &Place) -> Option<Self> {
        let root = place.attachments.parent()?.to_owned();
        let guest = match place.host {
            PlaceHost::Local => None,
            PlaceHost::Guest { workspace } => Some(workspace),
        };
        Some(Self {
            root,
            guest,
            later: true,
        })
    }

    /// Where `agent`'s picture goes, named for the SHA-256 of its bytes.
    fn path(&self, agent: AgentId, sha: &str, mime: &str) -> PathBuf {
        let name = format!("{sha}.{}", extension(mime));
        self.root.join(agent.to_string()).join(FOLDER).join(name)
    }
}

/// A picture for a container's guest to write where the host named it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForGuest {
    /// The container's workspace.
    pub workspace: WorkspaceId,
    /// The agent whose folder it goes in.
    pub agent: AgentId,
    /// Its media type.
    pub media_type: String,
    /// The picture, base64 encoded as the tool gave it.
    pub data: String,
}

/// A picture whose preview is still to be made, by the session off its
/// loop. A hostile picture can take seconds to decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForPreview {
    /// The agent whose timeline shows it.
    pub agent: AgentId,
    /// The attachment the preview is for.
    pub id: AttachmentId,
    /// Its media type.
    pub mime: &'static str,
    /// The picture.
    pub bytes: Vec<u8>,
}

/// What keeping pictures leaves for the session to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// Pictures for containers' guests to write, where the timeline says
    /// they are.
    pub guest: Vec<ForGuest>,
    /// Previews to make, which fill in on the timeline once they're done.
    pub previews: Vec<ForPreview>,
}

impl Kept {
    /// Moves everything in `other` here.
    pub fn append(&mut self, other: &mut Self) {
        self.guest.append(&mut other.guest);
        self.previews.append(&mut other.previews);
    }
}

/// The pictures in `content`, a tool result's blocks, each kept as a file
/// for `agent` where `pictures` says. One for a container goes on `kept`
/// for the driver to hand over, unless it's too large to send, when the
/// container's next history read writes it. A picture whose file can't be
/// written is still listed, with its preview.
pub fn keep(
    content: Option<&Value>,
    agent: AgentId,
    pictures: &Pictures,
    kept: &mut Kept,
) -> Vec<AttachmentRef> {
    let Some(content) = content.and_then(Value::as_array) else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(picture)
        .take(MAX_PER_RESULT)
        .map(|(mime, data, bytes)| {
            let sha = sha256_hex(&bytes);
            let path = pictures.path(agent, &sha, mime);
            match pictures.guest {
                None => write(&path, &bytes),
                Some(workspace) if data.len() <= MAX_PICTURE_LEN => kept.guest.push(ForGuest {
                    workspace,
                    agent,
                    media_type: mime.to_owned(),
                    data: data.to_owned(),
                }),
                Some(_) => {}
            }
            let id = AttachmentId::derive(&format!("tool-picture:{agent}:{sha}"));
            let size = bytes.len() as u64;
            let thumbnail = if pictures.later {
                kept.previews.push(ForPreview {
                    agent,
                    id,
                    mime,
                    bytes,
                });
                None
            } else {
                thumbnail::thumbnail_of(&bytes, Some(mime))
            };
            AttachmentRef {
                id,
                name: format!("image-{}.{}", &sha[..8], extension(mime)),
                kind: AttachmentKind::Image,
                bytes: size,
                mime: Some(mime.to_owned()),
                path: path.to_string_lossy().into_owned(),
                thumbnail,
            }
        })
        .collect()
}

/// Writes a picture the host handed over for `agent`, under `root`, where
/// [`keep`] named it. One MaxHQ doesn't keep is left out.
pub fn land(root: &Path, agent: AgentId, media_type: &str, data: &str) {
    let Some((mime, bytes)) = mime_of(media_type).zip(decode(data)) else {
        return;
    };
    let path = Pictures::here(root.to_owned()).path(agent, &sha256_hex(&bytes), mime);
    write(&path, &bytes);
}

/// Puts `images` on the item that shows a tool call, when it's one that
/// holds pictures.
pub fn show(body: &mut ItemBody, images: Vec<AttachmentRef>) {
    if images.is_empty() {
        return;
    }
    if let ItemBody::FileRead { images: shown, .. } | ItemBody::ToolCall { images: shown, .. } =
        body
    {
        *shown = images;
    }
}

/// The media type, base64 and bytes of one image block, or none for any
/// other block or a picture MaxHQ doesn't keep. Claude's blocks are the
/// API's, with a `source`, and Codex passes on MCP's, with the data on the
/// block itself.
fn picture(block: &Value) -> Option<(&'static str, &str, Vec<u8>)> {
    if block.get("type").and_then(Value::as_str) != Some("image") {
        return None;
    }
    let (media_type, data) = match block.get("source") {
        Some(source) => {
            if source.get("type").and_then(Value::as_str) != Some("base64") {
                return None;
            }
            (source.get("media_type")?, source.get("data")?)
        }
        None => (block.get("mimeType")?, block.get("data")?),
    };
    let mime = mime_of(media_type.as_str()?)?;
    let data = data.as_str()?;
    Some((mime, data, decode(data)?))
}

fn mime_of(media_type: &str) -> Option<&'static str> {
    Some(match media_type {
        "image/png" => "image/png",
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => return None,
    })
}

fn decode(data: &str) -> Option<Vec<u8>> {
    // Base64 takes four characters for every three bytes.
    if data.len() / 4 * 3 > MAX_BYTES {
        return None;
    }
    let bytes = STANDARD.decode(data).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// Writes `bytes` to `path` unless a file of that size is there already.
/// The name comes from the bytes, so that file holds them. It lands under
/// a name of its own first, so a reader never sees half of it.
fn write(path: &Path, bytes: &[u8]) {
    if let Err(error) = write_new(path, bytes) {
        tracing::debug!(%error, path = %path.display(), "couldn't keep a tool result's picture");
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == bytes.len() as u64) {
        return Ok(());
    }
    let folder = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(folder)?;
    write_atomic(path, bytes, Atomic::PLAIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use serde_json::json;

    fn png() -> Vec<u8> {
        let picture = image::RgbImage::from_pixel(4, 3, image::Rgb([200, 30, 30]));
        let mut out = io::Cursor::new(Vec::new());
        picture.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn content(blocks: Vec<Value>) -> Value {
        Value::Array(blocks)
    }

    fn image_block(mime: &str, bytes: &[u8]) -> Value {
        json!({
            "type": "image",
            "source": { "type": "base64", "media_type": mime, "data": STANDARD.encode(bytes) },
        })
    }

    #[test]
    fn a_picture_too_large_to_decode_is_kept_without_a_preview() {
        let name =
            "drive::pictures::tests::a_picture_too_large_to_decode_is_kept_without_a_preview";
        if crate::testing::apart(name) {
            return;
        }
        let dir = TempDir::new();
        let pictures = Pictures::here(dir.path().to_owned());
        let result = content(vec![image_block("image/png", &crate::testing::HUGE_PNG)]);
        let kept = keep(
            Some(&result),
            AgentId::generate(),
            &pictures,
            &mut Kept::default(),
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].thumbnail, None);
        assert!(Path::new(&kept[0].path).exists());
    }

    #[test]
    fn a_picture_lands_in_the_agents_folder_under_a_name_from_its_bytes() {
        let dir = TempDir::new();
        let pictures = Pictures::here(dir.path().to_owned());
        let agent = AgentId::derive("lead");
        let bytes = png();
        let blocks = content(vec![
            json!({ "type": "text", "text": "a screenshot" }),
            image_block("image/png", &bytes),
        ]);
        let mut handed = Kept::default();
        let kept = keep(Some(&blocks), agent, &pictures, &mut handed);
        assert_eq!(kept.len(), 1);
        assert!(handed.guest.is_empty());
        assert!(handed.previews.is_empty());
        let sha = sha256_hex(&bytes);
        let path = dir
            .path()
            .join(agent.to_string())
            .join(FOLDER)
            .join(format!("{sha}.png"));
        assert_eq!(kept[0].path, path.to_string_lossy());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(kept[0].bytes, bytes.len() as u64);
        assert_eq!(kept[0].mime.as_deref(), Some("image/png"));
        assert_eq!(kept[0].kind, AttachmentKind::Image);
        assert!(kept[0]
            .thumbnail
            .as_deref()
            .is_some_and(|url| url.starts_with("data:image/")));

        // Reading it again, as a history read does, lands on the same file.
        assert_eq!(keep(Some(&blocks), agent, &pictures, &mut handed), kept);
        let names: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn an_mcp_picture_lands_the_same_way() {
        let dir = TempDir::new();
        let pictures = Pictures::here(dir.path().to_owned());
        let agent = AgentId::derive("lead");
        let bytes = png();
        let mcp = content(vec![json!({
            "type": "image",
            "data": STANDARD.encode(&bytes),
            "mimeType": "image/png",
        })]);
        let api = content(vec![image_block("image/png", &bytes)]);
        let kept = keep(Some(&mcp), agent, &pictures, &mut Kept::default());
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read(&kept[0].path).unwrap(), bytes);
        assert_eq!(
            keep(Some(&api), agent, &pictures, &mut Kept::default()),
            kept
        );
    }

    #[test]
    fn a_running_drivers_picture_leaves_its_preview_for_later() {
        let dir = TempDir::new();
        let pictures = Pictures {
            later: true,
            ..Pictures::here(dir.path().to_owned())
        };
        let agent = AgentId::derive("lead");
        let bytes = png();
        let blocks = content(vec![image_block("image/png", &bytes)]);
        let mut handed = Kept::default();
        let kept = keep(Some(&blocks), agent, &pictures, &mut handed);
        assert_eq!(kept[0].thumbnail, None);
        assert_eq!(
            handed.previews,
            vec![ForPreview {
                agent,
                id: kept[0].id,
                mime: "image/png",
                bytes,
            }]
        );
    }

    #[test]
    fn a_container_picture_is_named_here_and_handed_to_its_guest() {
        let dir = TempDir::new();
        let workspace = WorkspaceId::derive("box");
        let pictures = Pictures {
            root: dir.path().join("in-the-container"),
            guest: Some(workspace),
            later: false,
        };
        let agent = AgentId::derive("lead");
        let bytes = png();
        let mut handed = Kept::default();
        let kept = keep(
            Some(&content(vec![image_block("image/jpeg", &bytes)])),
            agent,
            &pictures,
            &mut handed,
        );
        assert_eq!(kept.len(), 1);
        assert!(kept[0].path.ends_with(".jpg"));
        assert!(kept[0].path.starts_with(&*dir.path().to_string_lossy()));
        assert!(!dir.path().join("in-the-container").exists());
        assert!(kept[0].thumbnail.is_some());
        assert_eq!(
            handed.guest,
            vec![ForGuest {
                workspace,
                agent,
                media_type: "image/jpeg".into(),
                data: STANDARD.encode(&bytes),
            }]
        );

        // The guest lands it where the host named it.
        let inside = dir.path().join("in-the-container");
        land(
            &inside,
            agent,
            &handed.guest[0].media_type,
            &handed.guest[0].data,
        );
        assert_eq!(fs::read(&kept[0].path).unwrap(), bytes);
    }

    #[test]
    fn text_results_and_other_media_keep_nothing() {
        let pictures = Pictures::here(PathBuf::from("/nowhere"));
        let agent = AgentId::derive("lead");
        let mut handed = Kept::default();
        let text = json!("just text");
        assert!(keep(Some(&text), agent, &pictures, &mut handed).is_empty());
        assert!(keep(None, agent, &pictures, &mut handed).is_empty());
        let svg = content(vec![image_block("image/svg+xml", b"<svg/>")]);
        assert!(keep(Some(&svg), agent, &pictures, &mut handed).is_empty());
        let url = content(vec![json!({
            "type": "image",
            "source": { "type": "url", "url": "https://example.com/a.png" },
        })]);
        assert!(keep(Some(&url), agent, &pictures, &mut handed).is_empty());
        let broken = content(vec![json!({
            "type": "image",
            "source": { "type": "base64", "media_type": "image/png", "data": "not base64!" },
        })]);
        assert!(keep(Some(&broken), agent, &pictures, &mut handed).is_empty());
        let dir = TempDir::new();
        land(
            dir.path(),
            agent,
            "image/svg+xml",
            &STANDARD.encode(b"<svg/>"),
        );
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }
}
