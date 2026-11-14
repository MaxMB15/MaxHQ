// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Files an agent sends with MaxHQ's send tool: copied out of its folders
//! on this device or in its container, with a preview, into its
//! attachments folder.

use image::{Rgb, RgbImage};

use super::*;

/// The files `paths` that `place`'s agent sends in the call `call`.
async fn send(
    rig: &Rig,
    place: &Place,
    call: &str,
    paths: &[&str],
) -> Result<Vec<AttachmentRef>, String> {
    let paths = paths.iter().map(|path| (*path).to_owned()).collect();
    rig.transfers.collect(place, call, paths).await
}

/// Writes `data` at `path` under `root`, making its folders.
fn put(root: &Path, path: &str, data: &[u8]) -> PathBuf {
    let at = root.join(path);
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    fs::write(&at, data).unwrap();
    at
}

/// Whether this device's staging holds anything.
fn staging_is_empty(rig: &Rig) -> bool {
    fs::read_dir(rig.attachments().join("staged"))
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true)
}

#[tokio::test]
async fn a_send_from_the_device_lands_in_the_agents_folder_with_a_preview() {
    let rig = Rig::new();
    let chart = rig.local.cwd.join("out/chart.png");
    fs::create_dir_all(chart.parent().unwrap()).unwrap();
    RgbImage::from_pixel(64, 32, Rgb([200, 40, 40]))
        .save(&chart)
        .unwrap();
    put(&rig.local.cwd, "notes.txt", b"three notes");

    let sent = send(
        &rig,
        &rig.local,
        "call-1",
        &["out/chart.png", "./notes.txt"],
    )
    .await
    .unwrap();
    let [image, notes] = sent.as_slice() else {
        panic!("expected two files, got {sent:?}");
    };
    assert_eq!(image.name, "chart.png");
    assert_eq!(image.kind, AttachmentKind::Image);
    assert!(image
        .thumbnail
        .as_deref()
        .unwrap()
        .starts_with("data:image/"));
    assert_eq!(notes.name, "notes.txt");
    assert_eq!(notes.bytes, 11);
    assert_eq!(notes.thumbnail, None);
    for file in &sent {
        let at = PathBuf::from(&file.path);
        assert!(at.starts_with(&rig.local.attachments), "{}", file.path);
        assert!(at.starts_with(rig.local.attachments.join(file.id.to_string())));
    }
    assert_eq!(fs::read(&notes.path).unwrap(), b"three notes");
    // The agent's own copies stay where they were.
    assert!(chart.exists());
    assert!(staging_is_empty(&rig));
}

#[tokio::test]
async fn a_send_takes_files_from_the_agents_attachments_folder_too() {
    let rig = Rig::new();
    let kept = put(&rig.local.attachments, "earlier/report.pdf", b"%PDF-1.4");
    let sent = send(&rig, &rig.local, "call-1", &[kept.to_str().unwrap()])
        .await
        .unwrap();
    assert_eq!(sent[0].kind, AttachmentKind::Pdf);
    assert_eq!(fs::read(&sent[0].path).unwrap(), b"%PDF-1.4");
}

#[tokio::test]
async fn a_send_refuses_folders_links_and_paths_outside_and_sends_nothing() {
    let rig = Rig::new();
    put(&rig.local.cwd, "good.txt", b"good");
    put(&rig.local.cwd, "folder/inside.txt", b"inside");
    let outside = TempDir::new();
    let secret = put(outside.path(), "secret.txt", b"secret");
    symlink(&secret, rig.local.cwd.join("link.txt")).unwrap();

    for (path, said) in [
        (
            "folder",
            "the path is a folder. Send the files in it instead",
        ),
        ("link.txt", "the path is a link, which MaxHQ won't follow"),
        ("../secret.txt", "the path has a `..` part"),
        (
            secret.to_str().unwrap(),
            "the path is outside the agent's folders",
        ),
    ] {
        let refused = send(&rig, &rig.local, "call-1", &["good.txt", path])
            .await
            .unwrap_err();
        assert_eq!(refused, format!("{path}: {said}"));
    }
    let missing = send(&rig, &rig.local, "call-1", &["gone.txt"])
        .await
        .unwrap_err();
    assert!(missing.starts_with("gone.txt: "), "{missing}");
    // The file that was fine went nowhere either.
    assert!(!rig.local.attachments.exists());
    assert!(staging_is_empty(&rig));
}

#[tokio::test]
async fn a_send_run_again_lands_on_the_same_files() {
    let rig = Rig::new();
    put(&rig.local.cwd, "notes.txt", b"first");
    let first = send(&rig, &rig.local, "call-1", &["notes.txt"])
        .await
        .unwrap();
    put(&rig.local.cwd, "notes.txt", b"second");
    let again = send(&rig, &rig.local, "call-1", &["notes.txt"])
        .await
        .unwrap();
    assert_eq!(again[0].id, first[0].id);
    assert_eq!(again[0].path, first[0].path);
    assert_eq!(fs::read(&again[0].path).unwrap(), b"second");

    // Another call is another file.
    let other = send(&rig, &rig.local, "call-2", &["notes.txt"])
        .await
        .unwrap();
    assert_ne!(other[0].id, first[0].id);
}

#[tokio::test]
async fn a_send_from_a_container_comes_back_into_its_folder() {
    let rig = Rig::new();
    let data = pattern(300 * 1024);
    put(
        &rig.container.root.path().join("work/repo"),
        "out/data.csv",
        &data,
    );

    let sent = send(&rig, &rig.boxed, "call-1", &["out/data.csv"])
        .await
        .unwrap();
    let [file] = sent.as_slice() else {
        panic!("expected one file, got {sent:?}");
    };
    assert_eq!(file.name, "data.csv");
    assert_eq!(file.bytes, data.len() as u64);
    let prefix = format!("{ATTACHMENTS_DIR}/{}/", rig.boxed_agent);
    assert!(file.path.starts_with(&prefix), "{}", file.path);
    assert_eq!(fs::read(rig.container.local(&file.path)).unwrap(), data);
    assert!(staging_is_empty(&rig));

    // A link in the container isn't followed, and `..` never leaves.
    symlink(
        "/etc/hosts",
        rig.container.root.path().join("work/repo/hosts"),
    )
    .unwrap();
    let refused = send(&rig, &rig.boxed, "call-2", &["hosts"])
        .await
        .unwrap_err();
    assert!(refused.starts_with("hosts: "), "{refused}");
    let refused = send(&rig, &rig.boxed, "call-3", &["../../etc/hosts"])
        .await
        .unwrap_err();
    assert_eq!(refused, "../../etc/hosts: the path has a `..` part");
}

#[tokio::test]
async fn a_shown_page_lands_named_for_its_title_from_a_file_or_whole() {
    use maxhq_core::tools::{Page, MAX_VISUAL_BYTES};
    let rig = Rig::new();
    put(&rig.local.cwd, "out/usage.html", b"<p>usage</p>");
    let (page, text) = rig
        .transfers
        .show(
            &rig.local,
            "call-1",
            Page::Path("out/usage.html".to_owned()),
            "visual-usage.html".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(page.name, "visual-usage.html");
    assert_eq!(text, "<p>usage</p>");
    assert!(PathBuf::from(&page.path).starts_with(&rig.local.attachments));
    assert_eq!(fs::read(&page.path).unwrap(), b"<p>usage</p>");
    // The agent's own copy stays where it was.
    assert!(rig.local.cwd.join("out/usage.html").exists());

    let (whole, text) = rig
        .transfers
        .show(
            &rig.local,
            "call-2",
            Page::Html("<p>whole</p>".to_owned()),
            "visual-whole.html".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(whole.name, "visual-whole.html");
    assert_eq!(text, "<p>whole</p>");
    assert_eq!(fs::read(&whole.path).unwrap(), b"<p>whole</p>");
    assert_ne!(whole.id, page.id);

    // A page over the limit is refused with the limit, and leaves nothing.
    put(
        &rig.local.cwd,
        "out/big.html",
        &vec![b'x'; MAX_VISUAL_BYTES + 1],
    );
    let refused = rig
        .transfers
        .show(
            &rig.local,
            "call-3",
            Page::Path("out/big.html".to_owned()),
            "visual-big.html".to_owned(),
        )
        .await
        .unwrap_err();
    assert!(
        refused.starts_with("out/big.html: The page is 513 KB, over the 512 KB"),
        "{refused}"
    );
    assert!(staging_is_empty(&rig));
}

#[tokio::test]
async fn a_shown_page_from_a_container_comes_back_into_its_folder() {
    use maxhq_core::tools::Page;
    let rig = Rig::new();
    put(
        &rig.container.root.path().join("work/repo"),
        "out/usage.html",
        b"<p>usage</p>",
    );
    let (page, text) = rig
        .transfers
        .show(
            &rig.boxed,
            "call-1",
            Page::Path("out/usage.html".to_owned()),
            "visual-usage.html".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(page.name, "visual-usage.html");
    assert_eq!(text, "<p>usage</p>");
    let prefix = format!("{ATTACHMENTS_DIR}/{}/", rig.boxed_agent);
    assert!(page.path.starts_with(&prefix), "{}", page.path);
    assert!(page.path.ends_with("/visual-usage.html"), "{}", page.path);
    assert!(staging_is_empty(&rig));
}
