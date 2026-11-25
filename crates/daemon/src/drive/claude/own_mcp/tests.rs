// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime};

use super::*;
use crate::testing::TempDir;

fn server(mark: &str) -> Value {
    json!({ "type": "stdio", "command": "/bin/echo", "args": [mark] })
}

fn put(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn names(servers: &Map<String, Value>) -> Vec<&str> {
    let mut names: Vec<_> = servers.keys().map(String::as_str).collect();
    names.sort_unstable();
    names
}

fn real(path: &Path) -> String {
    fs_ext::canonicalize(path).unwrap().display().to_string()
}

/// A checkout at `root`, as git would leave it, with nothing in it.
fn checkout(root: &Path) {
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
}

/// A worktree of the checkout at `main`, at `tree`.
fn worktree(main: &Path, tree: &Path) {
    let admin = main.join(".git/worktrees/tree");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(tree).unwrap();
    fs::write(admin.join("HEAD"), "ref: refs/heads/tree\n").unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", tree.join(".git").display()),
    )
    .unwrap();
    fs::write(tree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
}

fn launch(cwd: &Path, args: &[&str]) -> Launch {
    Launch {
        session: SessionId::generate(),
        program: "claude".into(),
        args: args.iter().map(OsString::from).collect(),
        env: Vec::new(),
        env_remove: Vec::new(),
        secret_env: Vec::new(),
        run_as: None,
        cwd: cwd.to_owned(),
        framing: Framing::ClaudeStreamJson,
    }
}

fn restricted(cwd: &Path) -> Launch {
    let mut args = vec!["-p", "--output-format", "stream-json"];
    args.extend(folder_trust::RESTRICTED_ARGS);
    launch(cwd, &args)
}

fn folders_in(dir: &TempDir) -> Folders {
    Folders {
        files: dir.path().join("run/mcp"),
        sessions: dir.path().join("hold"),
    }
}

/// The file a launch names after `--mcp-config`.
fn named(launch: &Launch) -> Option<PathBuf> {
    let at = launch.args.iter().position(|arg| arg == MCP_CONFIG)?;
    launch.args.get(at + 1).map(PathBuf::from)
}

fn age(path: &Path, by: Duration) {
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - by).unwrap();
}

#[test]
fn the_config_is_in_the_config_folder_or_the_home_folder() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let config = dir.path().join("config");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(&config).unwrap();
    assert_eq!(config_file(None, &home), home.join(".claude.json"));
    assert_eq!(
        config_file(Some(&config), &home),
        config.join(".claude.json")
    );

    // An empty file older versions left doesn't count, as for the CLI.
    fs::write(home.join(".claude/.config.json"), "").unwrap();
    assert_eq!(config_file(None, &home), home.join(".claude.json"));
    fs::write(home.join(".claude/.config.json"), "{}").unwrap();
    assert_eq!(config_file(None, &home), home.join(".claude/.config.json"));
    assert_eq!(
        config_file(Some(&config), &home),
        config.join(".claude.json")
    );
    fs::write(config.join(".config.json"), "{}").unwrap();
    assert_eq!(
        config_file(Some(&config), &home),
        config.join(".config.json")
    );
}

#[test]
fn the_servers_are_yours_and_this_projects_and_not_others() {
    let dir = TempDir::new();
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let config = dir.path().join(".claude.json");
    put(
        &config,
        &json!({
            "mcpServers": {
                "everywhere": server("user"),
                "both": server("user"),
                "off-here": server("user"),
                SERVER: server("user"),
            },
            "projects": {
                real(&project): {
                    "mcpServers": { "here": server("local"), "both": server("local") },
                    "disabledMcpServers": ["off-here"],
                },
                "/elsewhere": { "mcpServers": { "elsewhere": server("other") } },
            },
            "oauthAccount": { "emailAddress": "someone@example.invalid" },
        }),
    );
    let found = servers(&config, &project, None).unwrap();
    assert_eq!(names(&found), ["both", "everywhere", "here"]);
    assert_eq!(found["both"], server("local"));

    // Elsewhere, only the user ones count.
    let other = dir.path().join("other");
    fs::create_dir_all(&other).unwrap();
    let found = servers(&config, &other, None).unwrap();
    assert_eq!(names(&found), ["both", "everywhere", "off-here"]);
    assert_eq!(found["both"], server("user"));
}

#[test]
fn local_servers_are_kept_by_checkout() {
    let dir = TempDir::new();
    let main = dir.path().join("main");
    checkout(&main);
    fs::create_dir_all(main.join("sub/deeper")).unwrap();
    let tree = dir.path().join("tree");
    worktree(&main, &tree);
    fs::create_dir_all(tree.join("sub")).unwrap();
    let plain = dir.path().join("plain/sub");
    fs::create_dir_all(&plain).unwrap();
    fs::create_dir_all(dir.path().join("links")).unwrap();
    fs_ext::symlink(&main, dir.path().join("links/main")).unwrap();

    let main_root = fs_ext::canonicalize(&main).unwrap();
    assert_eq!(project_root(&main), main_root);
    assert_eq!(project_root(&main.join("sub/deeper")), main_root);
    assert_eq!(project_root(&dir.path().join("links/main/sub")), main_root);
    // A linked worktree shares its main checkout's, as the CLI has it.
    assert_eq!(project_root(&tree.join("sub")), main_root);
    // Outside a checkout, it's the folder itself.
    assert_eq!(project_root(&plain), fs_ext::canonicalize(&plain).unwrap());
}

#[test]
fn a_worktree_that_names_another_checkout_falsely_keeps_its_own() {
    let dir = TempDir::new();
    let main = dir.path().join("main");
    checkout(&main);
    let tree = dir.path().join("tree");
    fs::create_dir_all(&tree).unwrap();
    // Its `.git` names the checkout, which doesn't name it back.
    let admin = main.join(".git");
    fs::write(tree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    assert_eq!(project_root(&tree), fs_ext::canonicalize(&tree).unwrap());
}

#[test]
fn a_config_that_is_missing_has_no_servers() {
    let dir = TempDir::new();
    let found = servers(&dir.path().join(".claude.json"), dir.path(), None).unwrap();
    assert!(found.is_empty());
}

// Unix owners and pipes at a path; Windows has neither.
#[cfg(unix)]
#[test]
fn a_config_that_is_not_yours_or_not_a_file_is_refused() {
    let dir = TempDir::new();
    let config = dir.path().join(".claude.json");
    put(&config, &json!({ "mcpServers": { "a": server("user") } }));
    let uid = fs_ext::uid_of(&fs::metadata(&config).unwrap()).unwrap();
    assert_eq!(
        names(&servers(&config, dir.path(), Some(uid)).unwrap()),
        ["a"]
    );
    assert!(servers(&config, dir.path(), Some(uid.wrapping_add(1))).is_err());

    // A pipe is an error at once, not a read that waits for a writer.
    let pipe = dir.path().join("pipe.json");
    let made = Command::new("mkfifo").arg(&pipe).status().unwrap();
    assert!(made.success());
    assert!(servers(&pipe, dir.path(), None).is_err());

    fs::write(&config, "{ not json").unwrap();
    assert!(servers(&config, dir.path(), None).is_err());
}

#[tokio::test]
async fn a_restricted_start_gets_a_private_file_of_your_servers() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let config = dir.path().join("config");
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    put(
        &config.join(".claude.json"),
        &json!({
            "mcpServers": { "everywhere": server("user") },
            "projects": { real(&project): { "mcpServers": { "here": server("local") } } },
        }),
    );
    // The home folder's config isn't the one this account uses.
    put(
        &home.join(".claude.json"),
        &json!({ "mcpServers": { "wrong": server("user") } }),
    );
    let mut start = restricted(&project);
    start
        .env
        .push(("CLAUDE_CONFIG_DIR".into(), home.clone().into()));
    start
        .env
        .push(("CLAUDE_CONFIG_DIR".into(), config.clone().into()));
    fill(&mut start, folders_in(&dir), &home).await;

    let file = named(&start).expect("the start names its servers");
    let folders = folders_in(&dir);
    assert_eq!(file, file_of(&folders, start.session));
    // Windows keeps the file private by its DACL, which the os crate's
    // own tests check, and has no modes to look at.
    if cfg!(unix) {
        let mode = |path: &Path| fsmode::mode_of(&fs::metadata(path).unwrap()) & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&folders.files), 0o711);
    }
    let written: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        written,
        json!({ "mcpServers": { "everywhere": server("user"), "here": server("local") } })
    );

    remove(&folders, start.session);
    assert!(!file.exists());
    // Forgetting it again is fine.
    remove(&folders, start.session);
}

#[tokio::test]
async fn other_starts_are_left_alone() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    put(
        &home.join(".claude.json"),
        &json!({ "mcpServers": { "a": server("user") } }),
    );
    let trusted = launch(dir.path(), &["-p"]);
    let mut codex = restricted(dir.path());
    codex.framing = Framing::JsonRpc;
    let mut named = restricted(dir.path());
    named
        .args
        .extend([MCP_CONFIG.into(), "/somewhere.json".into()]);
    // Only part of the restricted flags isn't a restricted start.
    let partly = launch(dir.path(), &["--strict-mcp-config"]);
    for start in [trusted, codex, named, partly] {
        let mut after = start.clone();
        fill(&mut after, folders_in(&dir), &home).await;
        assert_eq!(after, start);
    }
    assert!(!folders_in(&dir).files.exists());
}

#[tokio::test]
async fn a_start_without_servers_or_with_a_broken_config_names_no_file() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let mut start = restricted(dir.path());
    let before = start.clone();
    fill(&mut start, folders_in(&dir), &home).await;
    assert_eq!(start, before);

    put(
        &home.join(".claude.json"),
        &json!({ "mcpServers": { SERVER: server("user") } }),
    );
    fill(&mut start, folders_in(&dir), &home).await;
    assert_eq!(start, before);

    fs::write(home.join(".claude.json"), "{ not json").unwrap();
    fill(&mut start, folders_in(&dir), &home).await;
    assert_eq!(start, before);
}

#[tokio::test]
async fn a_start_clears_old_files_of_sessions_that_are_gone() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    put(
        &home.join(".claude.json"),
        &json!({ "mcpServers": { "a": server("user") } }),
    );
    let folders = folders_in(&dir);
    fs::create_dir_all(&folders.files).unwrap();
    let old = Duration::from_secs(3600);
    let file = |name: &str| folders.files.join(name);
    let (gone, starting, held) = (
        SessionId::generate(),
        SessionId::generate(),
        SessionId::generate(),
    );
    fs::create_dir_all(folders.sessions.join(held.to_string())).unwrap();
    for (session, by) in [(gone, old), (starting, Duration::ZERO), (held, old)] {
        let path = file_of(&folders, session);
        fs::write(&path, "{}").unwrap();
        age(&path, by);
    }
    fs::write(file("notes.txt"), "").unwrap();
    age(&file("notes.txt"), old);

    let mut start = restricted(dir.path());
    fill(&mut start, folders_in(&dir), &home).await;
    assert!(named(&start).is_some());
    assert!(!file_of(&folders, gone).exists());
    // One whose holder may not have made its folder yet stays.
    assert!(file_of(&folders, starting).exists());
    assert!(file_of(&folders, held).exists());
    assert!(file("notes.txt").exists());
}

/// The `claude` on `PATH`, if there is one. Windows' is a `claude.exe` or
/// `claude.cmd`, which this doesn't look for, so the test that needs it
/// is skipped there: its fake server is a shell script.
fn claude() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("claude"))
        .find(|file| fs::metadata(file).is_ok_and(|meta| fsmode::is_executable(file, &meta)))
}

/// A stdio MCP server that marks `$1` when it starts and answers just
/// enough to be connected.
const FAKE_SERVER: &str = r#"#!/bin/sh
touch "$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      v=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"0"}}}\n' "$id" "$v";;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"ping","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id";;
    *'"id":'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id";;
  esac
done
"#;

/// An API on localhost that turns every request away, so the CLI ends
/// without reaching anything else.
fn refusing_api() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
                    let header = line.trim_end().to_ascii_lowercase();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(value) = header.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let error =
                    br#"{"type":"error","error":{"type":"invalid_request_error","message":"no"}}"#;
                let head = format!(
                    "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n",
                    error.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(error);
            });
        }
    });
    port
}

/// Runs `claude` once in `cwd` and gives the names of the MCP servers its
/// first message lists.
fn run_claude(claude: &Path, home: &Path, cwd: &Path, port: u16, args: &[OsString]) -> Vec<String> {
    let path = env::join_paths([
        claude.parent().unwrap(),
        Path::new("/usr/bin"),
        Path::new("/bin"),
    ])
    .unwrap();
    let mut child = Command::new(claude)
        // The prompt goes first, since `--mcp-config` takes every path
        // after it.
        .args(["-p", "hi", "--output-format", "stream-json", "--verbose"])
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("ANTHROPIC_API_KEY", "sk-ant-not-a-real-key")
        .env("ANTHROPIC_BASE_URL", format!("http://127.0.0.1:{port}"))
        .env("NO_PROXY", "127.0.0.1")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("DISABLE_TELEMETRY", "1")
        .env("DISABLE_ERROR_REPORTING", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("CLAUDE_CODE_MAX_RETRIES", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let read = |out: Box<dyn io::Read + Send>| {
        std::thread::spawn(move || {
            BufReader::new(out)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<_>>()
        })
    };
    let lines = read(Box::new(child.stdout.take().unwrap()));
    let errors = read(Box::new(child.stderr.take().unwrap()));
    let until = Instant::now() + Duration::from_secs(90);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > until {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.wait();
    let lines = lines.join().unwrap();
    let init = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| line["type"] == "system" && line["subtype"] == "init");
    let Some(init) = init else {
        let errors = errors.join().unwrap();
        panic!("the CLI didn't say what it started with: {lines:?} {errors:?}");
    };
    let mut names: Vec<String> = init["mcp_servers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|server| server["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

/// The real CLI, in a home and a checkout of its own, with an API on
/// localhost that turns it away. Skipped when `claude` isn't on `PATH`.
#[tokio::test]
async fn the_real_cli_starts_your_servers_and_not_the_folders() {
    let Some(claude) = claude() else {
        eprintln!("skipped: claude isn't on PATH");
        return;
    };
    let port = refusing_api();
    for restrict in [false, true] {
        let dir = TempDir::new();
        let home = dir.path().join("home");
        let repo = dir.path().join("repo");
        checkout(&repo);
        let cwd = repo.join("sub");
        fs::create_dir_all(&cwd).unwrap();
        let script = dir.path().join("server.sh");
        fs::write(&script, FAKE_SERVER).unwrap();
        let mark = |name: &str| dir.path().join(format!("started-{name}"));
        let fake = |name: &str| json!({ "type": "stdio", "command": "/bin/sh", "args": [&script, mark(name)] });
        put(
            &home.join(".claude.json"),
            &json!({
                "hasCompletedOnboarding": true,
                "mcpServers": { "usersrv": fake("user") },
                "projects": { real(&repo): {
                    "mcpServers": { "localsrv": fake("local") },
                    "hasTrustDialogAccepted": true,
                    "enabledMcpjsonServers": ["projsrv"],
                    "enableAllProjectMcpServers": true,
                } },
            }),
        );
        put(
            &cwd.join(".mcp.json"),
            &json!({ "mcpServers": { "projsrv": fake("project") } }),
        );

        let mut start = launch(&cwd, &[]);
        if restrict {
            start
                .args
                .extend(folder_trust::RESTRICTED_ARGS.map(OsString::from));
        }
        fill(&mut start, folders_in(&dir), &home).await;
        assert_eq!(named(&start).is_some(), restrict);

        let listed = run_claude(&claude, &home, &cwd, port, &start.args);
        let started = |name: &str| mark(name).exists();
        assert!(
            started("user") && started("local"),
            "{restrict}: {listed:?}"
        );
        assert!(
            listed.contains(&"usersrv".to_owned()),
            "{restrict}: {listed:?}"
        );
        assert!(
            listed.contains(&"localsrv".to_owned()),
            "{restrict}: {listed:?}"
        );
        if restrict {
            assert!(!started("project"), "{listed:?}");
            assert!(!listed.contains(&"projsrv".to_owned()), "{listed:?}");
        } else {
            // Without the flags, the folder's server would have started.
            assert!(started("project"), "{listed:?}");
            assert!(listed.contains(&"projsrv".to_owned()), "{listed:?}");
        }
    }
}
