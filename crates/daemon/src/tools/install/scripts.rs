// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// The lock follows MIT-licensed code, see THIRD_PARTY_NOTICES: a file made
// with noclobber that holds its owner's PID. The publish follows the same
// code: a finished version gets its `.install-complete` record, moves into
// place by a rename, and the active version flips by a rename too.

//! The shell scripts the installer runs in a place, over [`Exec`]. They
//! are plain POSIX sh, so they run the same on a Mac, a Linux host and a
//! container. Each takes its paths as arguments, never in its text.
//!
//! [`Exec`]: crate::recipe::Exec

/// Takes the tool's lock. It takes the tool's folder and the owner's
/// token, and exits 3 printing the owner when someone else holds it. The
/// lock is made with noclobber, so only one taker wins.
pub const LOCK: &str = r#"set -eu
mkdir -p "$1"
if (set -C; printf '%s\n' "$2" > "$1/.lock") 2>/dev/null; then exit 0; fi
cat "$1/.lock" 2>/dev/null || true
exit 3
"#;

/// Drops the tool's lock when the token given still holds it. It takes the
/// tool's folder and the token.
pub const UNLOCK: &str = r#"if [ "$(cat "$1/.lock" 2>/dev/null || true)" = "$2" ]; then rm -f "$1/.lock"; fi
"#;

/// Clears what an earlier install left half done and makes a fresh staging
/// folder. It takes the tool's folder and the staging folder's name. The
/// lock is held, so any other staging folder is dead.
pub const STAGE: &str = r#"set -eu
mkdir -p "$1"
for old in "$1"/.staging-*; do
  if [ -e "$old" ]; then rm -rf "$old"; fi
done
mkdir -p "$1/$2/dl" "$1/$2/files"
"#;

/// Unpacks the downloads into the staging folder's `files`. It takes the
/// staging folder, the target and the downloads' names in `dl`. A tar or
/// zip that holds one folder is unpacked from inside it, a Mac bundle's
/// `Contents/Home` is unpacked from inside that, as a JDK ships, and a name
/// that ends in `-<target>` loses that end, so `codex-<target>` is `codex`.
/// It prints the programs it unpacked, as `./name` or `./bin/name`.
pub const UNPACK: &str = r#"set -eu
s="$1"; t="$2"; shift 2
for a in "$@"; do
  x="$s/x"
  rm -rf "$x"
  mkdir "$x"
  case "$a" in
    *.tar.gz|*.tgz|*.tar.xz|*.tar.bz2|*.tbz2|*.tar) tar -xf "$s/dl/$a" -C "$x" ;;
    *.zip) unzip -q "$s/dl/$a" -d "$x" ;;
    *) cp "$s/dl/$a" "$x/$a"; chmod 755 "$x/$a" ;;
  esac
  src="$x"; n=0; one=
  for e in "$x"/* "$x"/.[!.]*; do
    if [ -e "$e" ]; then n=$((n + 1)); one="$e"; fi
  done
  if [ "$n" = 1 ] && [ -d "$one" ]; then src="$one"; fi
  if [ -d "$src/Contents/Home" ]; then src="$src/Contents/Home"; fi
  for e in "$src"/* "$src"/.[!.]*; do
    if [ ! -e "$e" ]; then continue; fi
    b="${e##*/}"
    b="${b%-"$t"}"
    rm -rf "$s/files/$b"
    mv "$e" "$s/files/$b"
  done
  rm -rf "$x" "$s/dl/$a"
done
cd "$s/files"
find . -maxdepth 2 -type f -perm -100 | sort
"#;

/// Exits 0 when a version's folder is whole: it has its record and every
/// program the record names is there and runs. It takes the folder.
pub const CHECK: &str = r#"[ -f "$1/.install-complete" ] || exit 1
while IFS= read -r line; do
  case "$line" in
    "entry "*)
      e="$1/${line#entry }"
      if [ ! -f "$e" ] || [ ! -x "$e" ]; then exit 1; fi
      ;;
  esac
done < "$1/.install-complete"
"#;

/// Publishes a staged version and its links. It takes the tool's folder,
/// the staging folder's name, the version, the folder for links, `1` to
/// replace whatever holds a link's name or `0` to leave it, the record,
/// and then pairs of a link's name and the program it runs inside the
/// version. Without a staged `files` the version is already in place, and
/// only `current` and the links move. It keeps the new version and the
/// one before it, and prints `linked <path>` or `kept <path>` per link.
pub const PUBLISH: &str = r#"set -eu
d="$1"; s="$d/$2"; v="$3"; b="$4"; r="$5"; rec="$6"; shift 6
flip() { mv -fT "$1" "$2" 2>/dev/null || mv -fh "$1" "$2"; }
if [ -d "$s/files" ]; then
  printf '%s\n' "$rec" > "$s/files/.install-complete"
  rm -rf "$d/$v"
  mv "$s/files" "$d/$v"
fi
rm -rf "$s"
prev="$(readlink "$d/current" 2>/dev/null || true)"
rm -f "$d/.current.new"
ln -s "$v" "$d/.current.new"
flip "$d/.current.new" "$d/current"
mkdir -p "$b"
: > "$d/.links.new"
while [ $# -ge 2 ]; do
  l="$b/$1"; e="$d/current/$2"; shift 2
  cur=
  if [ -L "$l" ]; then cur="$(readlink "$l")"; fi
  if { [ ! -e "$l" ] && [ ! -L "$l" ]; } || [ "$cur" = "$e" ] || [ "$r" = 1 ]; then
    if [ "$cur" != "$e" ]; then
      rm -f "$l.maxhq-new"
      ln -s "$e" "$l.maxhq-new"
      flip "$l.maxhq-new" "$l"
    fi
    printf '%s\n' "$l" >> "$d/.links.new"
    echo "linked $l"
  else
    echo "kept $l"
  fi
done
mv -f "$d/.links.new" "$d/links"
for old in "$d"/*; do
  n="${old##*/}"
  case "$n" in
    "$v"|"$prev"|current|links|script) ;;
    *) rm -rf "$old" ;;
  esac
done
"#;

/// Runs a tool's own install script from its home. It takes the script
/// and what the script takes.
pub const RUN: &str = r#"cd "$HOME" && exec bash "$@"
"#;

/// Records an install the tool's own script made. It takes the tool's
/// folder, the version and the link the script made.
pub const FINISH: &str = r#"set -eu
for old in "$1"/.staging-*; do
  if [ -e "$old" ]; then rm -rf "$old"; fi
done
printf '%s\n' "$2" > "$1/script"
printf '%s\n' "$3" > "$1/links"
"#;

/// Removes a tool MaxHQ installed: its links, when they still point into
/// its folder or its own script made them, then the folder. It takes the
/// folder, and exits 4 when MaxHQ installed nothing there, leaving only
/// what the lock made.
pub const REMOVE: &str = r#"d="$1"
if [ ! -L "$d/current" ] && [ ! -f "$d/script" ]; then
  rm -f "$d/.lock"
  rmdir "$d" 2>/dev/null || true
  exit 4
fi
if [ -f "$d/links" ]; then
  while IFS= read -r l; do
    if [ -f "$d/script" ]; then
      rm -f "$l"
    elif [ -L "$l" ]; then
      case "$(readlink "$l")" in "$d"/*) rm -f "$l" ;; esac
    fi
  done < "$d/links"
fi
rm -rf "$d"
"#;
