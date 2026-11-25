// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Setup of a Windows PC over the OpenSSH Server Windows comes with.
//!
//! Each step runs in Windows PowerShell, sent as `-EncodedCommand`, base64
//! of the script's UTF-16LE. That one plain word reaches PowerShell the
//! same through cmd, which OpenSSH starts commands with by default, and
//! through a `DefaultShell` of PowerShell or `sh`, so no quoting depends
//! on which one the PC has. Values go into the script as PowerShell
//! literals.
//!
//! `maxd.exe` goes in `%LOCALAPPDATA%\MaxHQ\versions\<build>`, and the
//! junction `bin\current` points at the build in use, so the key line and
//! the service name one path through every update.

use std::collections::BTreeMap;

use maxhq_core::authorized_keys::{self, WINDOWS_MAXD};
use maxhq_core::shell::powershell::{encoded, literal};

use super::{parse_probe, stop, Prepared, Run, Setup, SetupBinary, SetupError, Step};

/// What turns on the SSH server a Windows PC needs, for a host that
/// doesn't answer.
pub const SSHD_STEPS: &str = "On a Windows PC, turn on OpenSSH Server under Settings > System > \
     Optional features, or run this in PowerShell as administrator: \
     Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0; Start-Service sshd; \
     Set-Service -Name sshd -StartupType Automatic";

/// The longest command cmd reads, which a step's command must fit in when
/// OpenSSH starts it through cmd.
const CMD_LINE_MAX: usize = 8191;

/// What the probe prints, one `key=value` a line: the OS, the machine's
/// own architecture (not the one PowerShell runs as), the folders, the
/// user, whether they're an administrator, OpenSSH's `DefaultShell` and
/// the build of the `maxd.exe` there.
pub(super) const PROBE: &str = r#"$ProgressPreference = 'SilentlyContinue'
"os=$env:OS"
"arch=$((Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment').PROCESSOR_ARCHITECTURE)"
"home=$env:USERPROFILE"
"local=$env:LOCALAPPDATA"
"user=$env:USERNAME"
$admin = (& "$env:SystemRoot\System32\whoami.exe" /groups /fo csv /nh) -match '"S-1-5-32-544"'
"admin=$(if ($admin) { 'yes' } else { 'no' })"
"shell=$((Get-ItemProperty 'HKLM:\SOFTWARE\OpenSSH' -ErrorAction SilentlyContinue).DefaultShell)"
$maxd = Join-Path $env:LOCALAPPDATA 'MaxHQ\bin\current\maxd.exe'
if (Test-Path -LiteralPath $maxd) { "build=$(& $maxd --build 2>$null)" }
"#;

/// Installs the `maxd.exe` that comes on stdin into `versions\<build>`,
/// after checking its sum, and points `bin\current` at it. A junction
/// can't be swapped in one step, so a new one is made beside it, the old
/// one removed and the new one renamed. Removing one that isn't there, or
/// points nowhere, may fail, and the rename then says what's in the way.
/// Runs after `$sum` and `$build` are set.
pub(super) const INSTALL: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = Join-Path $env:LOCALAPPDATA 'MaxHQ'
$v = Join-Path $root "versions\$build"
New-Item -ItemType Directory -Force -Path $v | Out-Null
$part = Join-Path $v 'maxd.exe.part'
$out = [IO.File]::Create($part)
try { [Console]::OpenStandardInput().CopyTo($out) } finally { $out.Close() }
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $part).Hash.ToLowerInvariant() -ne $sum) {
  Remove-Item -Force -LiteralPath $part
  [Console]::Error.WriteLine('the upload does not match its sha256')
  exit 3
}
Move-Item -Force -LiteralPath $part -Destination (Join-Path $v 'maxd.exe')
$bin = Join-Path $root 'bin'
New-Item -ItemType Directory -Force -Path $bin | Out-Null
$next = Join-Path $bin 'current.new'
$cur = Join-Path $bin 'current'
try { [IO.Directory]::Delete($next) } catch {}
New-Item -ItemType Junction -Path $next -Target $v | Out-Null
try { [IO.Directory]::Delete($cur) } catch {}
Rename-Item -LiteralPath $next -NewName 'current'
"#;

/// Runs the `maxd.exe` in `bin\current` with the arguments set before it,
/// and exits as it does.
pub(super) const RUN_MAXD: &str = r#"$ProgressPreference = 'SilentlyContinue'
& (Join-Path $env:LOCALAPPDATA 'MaxHQ\bin\current\maxd.exe') @list
exit $LASTEXITCODE
"#;

/// A remote command that runs `script` in Windows PowerShell from any
/// shell OpenSSH starts it with.
pub(super) fn powershell(script: &str) -> String {
    let command = format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        encoded(script)
    );
    debug_assert!(
        command.len() < CMD_LINE_MAX,
        "a setup step is too long for cmd"
    );
    command
}

/// `text` as an argument Windows PowerShell 5.1 passes to a program
/// whole. It quotes an argument with a space but escapes nothing in it,
/// so a straight double quote becomes a curly one and a backslash that
/// would come before its closing quote goes.
pub(super) fn native_arg(text: &str) -> String {
    text.replace('"', "\u{201D}")
        .trim_end_matches('\\')
        .to_owned()
}

/// The script that runs `maxd.exe` with `args`.
pub(super) fn run_maxd(args: &[&str]) -> String {
    let list: Vec<String> = args.iter().map(|arg| literal(&native_arg(arg))).collect();
    format!("$list = @({})\n{RUN_MAXD}", list.join(", "))
}

/// The script that installs a `maxd.exe` with sha256 `sum` as `build`.
pub(super) fn install(sum: &str, build: &str) -> String {
    format!(
        "$sum = {}\n$build = {}\n{INSTALL}",
        literal(sum),
        literal(build)
    )
}

/// The probe's facts, when it ran on Windows.
pub(super) fn facts(output: &str) -> Option<BTreeMap<String, String>> {
    let facts = parse_probe(output);
    (facts.get("os").map(String::as_str) == Some("Windows_NT")).then_some(facts)
}

/// Whether `uname -sm` came from a POSIX layer on Windows, such as Git
/// for Windows' `sh`, rather than from a Unix system.
pub(super) fn posix_layer(uname: &str) -> bool {
    ["MINGW", "MSYS", "CYGWIN"]
        .iter()
        .any(|layer| uname.starts_with(layer))
}

/// The Rust target for the `PROCESSOR_ARCHITECTURE` a Windows PC has.
pub fn target_for_windows(arch: &str) -> Option<&'static str> {
    match arch.to_ascii_uppercase().as_str() {
        "AMD64" => Some("x86_64-pc-windows-msvc"),
        "ARM64" => Some("aarch64-pc-windows-msvc"),
        _ => None,
    }
}

/// Why MaxHQ's key line can't run under `shell`, the `DefaultShell`
/// OpenSSH starts commands with, when it names `maxd`. The line suits cmd,
/// which runs when none is set. PowerShell runs it too while the path has
/// no space, since it reads a quoted program as text.
pub fn shell_refusal(shell: &str, maxd: &str) -> Option<String> {
    let name = shell
        .trim()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let fix = "Remove the DefaultShell setting so OpenSSH uses cmd: in PowerShell as \
               administrator, run Remove-ItemProperty -Path HKLM:\\SOFTWARE\\OpenSSH -Name \
               DefaultShell";
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "" | "cmd" => None,
        "powershell" | "pwsh" if !maxd.contains(' ') => None,
        "powershell" | "pwsh" => Some(format!(
            "OpenSSH on this PC runs commands through PowerShell, which can't start {maxd} \
             from MaxHQ's key line, since its path has a space. {fix}"
        )),
        _ => Some(format!(
            "OpenSSH on this PC runs commands through {}, and MaxHQ's key line runs maxd \
             through cmd or PowerShell. {fix}",
            shell.trim()
        )),
    }
}

impl<R: Run> Setup<'_, R> {
    /// The Windows probe's facts, or none when the host isn't Windows.
    pub(super) async fn windows_facts(&self) -> Option<BTreeMap<String, String>> {
        let output = self.ssh(vec![powershell(PROBE)], None).await.ok()?;
        facts(&output)
    }

    /// The rest of [`Setup::prepare`] on a Windows PC, from its probe's
    /// facts.
    pub(super) async fn prepare_windows(
        self,
        facts: BTreeMap<String, String>,
        binary: impl Fn(&str) -> Option<SetupBinary>,
        step: &(dyn Fn(Step) + Sync),
        (host_keys, host_keys_confirmed): (Vec<String>, bool),
    ) -> Result<Prepared, SetupError> {
        let fact = |key: &str| facts.get(key).map_or("", String::as_str);
        let Some(target) = target_for_windows(fact("arch")) else {
            return Err(stop(format!(
                "MaxHQ can't run on Windows on {} yet",
                fact("arch")
            )));
        };
        let local = fact("local").trim_end_matches('\\');
        if local.is_empty() {
            return Err(stop("The PC didn't say where its LOCALAPPDATA folder is"));
        }
        let maxd = format!("{local}\\{WINDOWS_MAXD}");
        if let Some(why) = shell_refusal(fact("shell"), &maxd) {
            return Err(stop(why));
        }
        let public = self.public_key()?;
        let line = authorized_keys::windows_line(&maxd, &public, None).map_err(stop)?;
        let binary = super::checked(binary(target), target)?;

        step(Step::Install);
        let build = &binary.sha256[..16];
        if fact("build") != build {
            let bytes = std::fs::read(&binary.path)
                .map_err(|error| stop(format!("Reading maxd failed: {error}")))?;
            self.ssh(
                vec![powershell(&install(&binary.sha256, build))],
                Some(bytes),
            )
            .await?;
        }
        self.ssh(vec![powershell(&run_maxd(&["service", "install"]))], None)
            .await?;

        // The key's type and key go as two words, which maxd reads as one
        // key, and an empty label as MaxHQ, since Windows PowerShell drops
        // an empty argument.
        let key = super::key_of(&public);
        let mut args: Vec<&str> = ["authorize-key"]
            .into_iter()
            .chain(key.split(' '))
            .collect();
        let label = self.app_key.as_ref().map(|(_, label)| native_arg(label));
        if let (Some((app_key, _)), Some(label)) = (&self.app_key, &label) {
            let label = match label.trim() {
                "" => "MaxHQ",
                label => label,
            };
            args.extend(["--app", app_key, label]);
        }
        self.ssh(vec![powershell(&run_maxd(&args))], None).await?;
        step(Step::Key { line });

        Ok(Prepared {
            host: self.host,
            home: fact("home").to_owned(),
            target: target.to_owned(),
            host_keys,
            host_keys_confirmed,
            // The logon task runs while you're logged in.
            linger: false,
        })
    }
}
