// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Windows's rules, written over text so they're tested on any system:
//! `PATH` split at `;`, a program is a file whose extension `PATHEXT`
//! names, and an npm `.cmd` shim is looked through to what it starts.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{find_in, Env, Fs, Launch, Rules};

/// What Windows uses when `PATHEXT` is unset.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The extensions `pathext` names, in lower case with their dot.
fn extensions(pathext: Option<&OsStr>) -> Vec<String> {
    let parse = |text: &str| -> Vec<String> {
        text.split(';')
            .map(str::trim)
            .filter(|ext| ext.len() > 1 && ext.starts_with('.'))
            .map(str::to_ascii_lowercase)
            .collect()
    };
    let named = pathext
        .and_then(OsStr::to_str)
        .map(parse)
        .unwrap_or_default();
    if named.is_empty() {
        parse(DEFAULT_PATHEXT)
    } else {
        named
    }
}

/// The last part of the Windows path `path`.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// The extension of the file name `name`, in lower case with its dot. A
/// name that starts with its only dot has none.
fn extension(name: &str) -> Option<String> {
    name.rfind('.')
        .filter(|&dot| dot > 0)
        .map(|dot| name[dot..].to_ascii_lowercase())
}

/// The extension of the file `path` names.
fn extension_of(path: &Path) -> Option<String> {
    extension(file_name(&path.to_string_lossy()))
}

/// `dir` and `file` joined with a backslash.
fn join(dir: &Path, file: &str) -> PathBuf {
    let mut path = dir.as_os_str().to_owned();
    if !dir.to_string_lossy().ends_with(['\\', '/']) {
        path.push("\\");
    }
    path.push(file);
    PathBuf::from(path)
}

/// Whether a file at `path` may run: its extension is one `pathext` names.
pub fn runs(path: &Path, pathext: Option<&OsStr>) -> bool {
    extension_of(path).is_some_and(|ext| extensions(pathext).contains(&ext))
}

/// The folders on `path`, split at `;` outside double quotes, with the
/// quotes taken out, as Windows reads it.
pub fn split_paths(path: &OsStr) -> Vec<PathBuf> {
    let Some(text) = path.to_str() else {
        // Only a Windows host has a `PATH` that isn't Unicode, and there
        // the standard library splits it the same way.
        return std::env::split_paths(path).collect();
    };
    let mut dirs = Vec::new();
    let mut dir = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => dirs.push(std::mem::take(&mut dir)),
            c => dir.push(c),
        }
    }
    dirs.push(dir);
    dirs.into_iter()
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Whether `dir` is absolute on Windows: a drive and a separator, or a
/// share. `\dir` is on the current drive, so it isn't.
pub fn is_absolute(dir: &Path) -> bool {
    let text = dir.to_string_lossy();
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || text.starts_with(r"\\") || text.starts_with("//")
}

/// The files `name` may be in `dir`: the name as it is when it already
/// has an extension `PATHEXT` names, and otherwise the name with `.exe`,
/// then with each other extension in `PATHEXT`'s order.
pub fn candidates(dir: &Path, name: &str, pathext: Option<&OsStr>) -> Vec<PathBuf> {
    let extensions = extensions(pathext);
    if extension(name).is_some_and(|ext| extensions.contains(&ext)) {
        return vec![join(dir, name)];
    }
    std::iter::once(".exe")
        .chain(
            extensions
                .iter()
                .map(String::as_str)
                .filter(|ext| *ext != ".exe"),
        )
        .map(|ext| join(dir, &format!("{name}{ext}")))
        .collect()
}

/// How to start `found`, looking through an npm shim.
pub fn launch(found: &Path, env: &Env, fs: &impl Fs) -> Launch {
    shim(found, env, fs).unwrap_or_else(|| Launch::direct(found))
}

/// What the npm shim `found` starts, or `None` when it isn't one this can
/// read, or the program it names isn't there.
fn shim(found: &Path, env: &Env, fs: &impl Fs) -> Option<Launch> {
    if !matches!(extension_of(found).as_deref(), Some(".cmd" | ".bat")) {
        return None;
    }
    let text = found.to_string_lossy();
    let dir = &text[..text.rfind(['\\', '/'])?];
    let target = PathBuf::from(shim_target(&fs.read_text(found)?, dir)?);
    fs.file_mode(&target)?;
    match extension_of(&target).as_deref() {
        // A shim that starts another batch file gains nothing.
        Some(".cmd" | ".bat") => None,
        Some(_) if runs(&target, env.pathext.as_deref()) => Some(Launch::direct(target)),
        // A script: npm puts `node.exe` beside the shim when it installed
        // its own, and otherwise the shim runs the one on `PATH`.
        _ => {
            let beside = join(Path::new(dir), "node.exe");
            let node = if fs.file_mode(&beside).is_some() {
                beside
            } else {
                find_in(Rules::Windows, "node", &[], env, fs)
                    .filter(|node| extension_of(node).as_deref() == Some(".exe"))?
            };
            Some(Launch {
                program: node,
                args: vec![target.into_os_string()],
            })
        }
    }
}

/// The file an npm shim's text `text` starts, with the shim's folder `dir`
/// put in for `%dp0%` or `%~dp0`. npm's shims, old and new, and pnpm's,
/// all end their command with `%*`, the caller's arguments, after the
/// quoted path of what they start.
fn shim_target(text: &str, dir: &str) -> Option<String> {
    let line = text.lines().rev().find(|line| line.contains("%*"))?;
    let named = line
        .split('"')
        .skip(1)
        .step_by(2)
        .filter_map(|quoted| {
            quoted
                .strip_prefix("%dp0%")
                .or_else(|| quoted.strip_prefix("%~dp0"))
        })
        .last()?;
    let rest = named.trim_start_matches(['\\', '/']);
    // Another variable is one this can't expand.
    if rest.is_empty() || rest.contains('%') {
        return None;
    }
    Some(format!("{dir}\\{rest}"))
}
