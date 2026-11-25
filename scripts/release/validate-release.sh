#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Checks that a published release's downloads work: every URL in latest.json
# and its .sig, the same for latest-v2.json when the release has one (after
# a change of the update key), which lists the Windows and Linux installers
# too, the versionless DMGs the README links to, SHA256SUMS and each maxd it
# lists, both Linux ones among them, the model catalog and its .sig, and
# mobile.json with the APK and App Bundle when it names an Android build.
#
#   sh scripts/release/validate-release.sh [<owner>/<repo>] [tag]
#
# The repo defaults to $GITHUB_REPOSITORY, then to the origin remote's. The
# tag defaults to the latest release. MAXHQ_RELEASE_BASE, for the tests,
# replaces the release's download URL. Exits non-zero when a URL doesn't
# answer 200.

set -eu

repo=${1:-${GITHUB_REPOSITORY:-}}
if [ -z "$repo" ]; then
	repo=$(git remote get-url origin | sed -E 's#^(git@github\.com:|https://github\.com/)##; s#\.git$##')
fi
tag=${2:-latest}
if [ -n "${MAXHQ_RELEASE_BASE:-}" ]; then
	base=$MAXHQ_RELEASE_BASE
elif [ "$tag" = latest ]; then
	base=https://github.com/$repo/releases/latest/download
else
	base=https://github.com/$repo/releases/download/$tag
fi

echo "=== $repo, release $tag ==="
failed=0
check() {
	code=$(curl -sIL -o /dev/null -w '%{http_code}' "$2")
	if [ "$code" = 200 ]; then
		echo "  $1: OK"
	else
		echo "  $1: HTTP $code $2"
		failed=1
	fi
}

json=$(mktemp)
trap 'rm -f "$json"' EXIT

# Checks one update manifest that answered 200 into $json.
manifest() {
	# One "platforms url signed" line per download, since an installer's
	# platform keys share its URL. signed is 0 when a signature is empty.
	platforms=$(node -e '
		const m = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
		console.error(`version ${m.version}`);
		const urls = new Map();
		for (const [name, p] of Object.entries(m.platforms ?? {})) {
			const entry = urls.get(p.url) ?? { names: [], signed: 1 };
			entry.names.push(name);
			if (!(p.signature ?? "").length) entry.signed = 0;
			urls.set(p.url, entry);
		}
		for (const [url, { names, signed }] of urls) console.log(`${names.join(",")} ${url} ${signed}`);
	' "$json")
	if [ -z "$platforms" ]; then
		echo "FAIL: $1 lists no platforms"
		exit 1
	fi
	while read -r platform url signed; do
		echo "--- $1, $platform"
		check download "$url"
		check .sig "$url.sig"
		if [ "$signed" = 0 ]; then
			echo "  signature in $1: empty"
			failed=1
		fi
	done <<EOF
$platforms
EOF
}

code=$(curl -sL -o "$json" -w '%{http_code}' "$base/latest.json")
if [ "$code" != 200 ]; then
	echo "FAIL: latest.json answered HTTP $code"
	exit 1
fi
manifest latest.json

# Only releases from a change of the update key on have latest-v2.json.
code=$(curl -sL -o "$json" -w '%{http_code}' "$base/latest-v2.json")
if [ "$code" = 200 ]; then
	manifest latest-v2.json
elif [ "$code" = 404 ]; then
	echo "--- latest-v2.json: none (no update key change yet)"
else
	echo "--- latest-v2.json: HTTP $code"
	failed=1
fi

echo "--- downloads"
for name in MaxHQ_aarch64.dmg MaxHQ_x64.dmg SHA256SUMS model-catalog.json model-catalog.json.sig; do
	check "$name" "$base/$name"
done

# Each maxd SHA256SUMS lists: both Linux ones, and each Windows one that
# built.
echo "--- maxd"
sums=$(curl -sfL "$base/SHA256SUMS" || true)
for name in maxd-x86_64-unknown-linux-musl maxd-aarch64-unknown-linux-musl; do
	if ! printf '%s\n' "$sums" | grep -Eq "^[0-9a-f]{64} [ *]$name\$"; then
		echo "  SHA256SUMS: has no $name"
		failed=1
	fi
done
while read -r sum name; do
	[ -n "$sum" ] || continue
	check "${name#\*}" "$base/${name#\*}"
done <<EOF
$sums
EOF

# Releases from before the phone builds have no mobile.json.
code=$(curl -sL -o "$json" -w '%{http_code}' "$base/mobile.json")
if [ "$code" = 200 ]; then
	apk=$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).android?.apk_url ?? ""' "$json")
	if [ -n "$apk" ]; then
		echo "--- mobile.json, Android"
		check MaxHQ.apk "$apk"
		check MaxHQ.aab "$base/MaxHQ.aab"
	else
		echo "--- mobile.json: no Android build"
	fi
elif [ "$code" = 404 ]; then
	echo "--- mobile.json: none"
else
	echo "--- mobile.json: HTTP $code"
	failed=1
fi

if [ "$failed" = 0 ]; then
	echo "=== all checks passed ==="
else
	echo "=== some checks failed ==="
	exit 1
fi
