#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Checks that a published release's downloads work: every URL in latest.json
# and its .sig, the versionless DMGs the README links to, the Linux maxd
# binaries and SHA256SUMS.
#
#   sh scripts/release/validate-release.sh [<owner>/<repo>] [tag]
#
# The repo defaults to $GITHUB_REPOSITORY, then to the origin remote's. The
# tag defaults to the latest release. Exits non-zero when a URL doesn't
# answer 200.

set -eu

repo=${1:-${GITHUB_REPOSITORY:-}}
if [ -z "$repo" ]; then
	repo=$(git remote get-url origin | sed -E 's#^(git@github\.com:|https://github\.com/)##; s#\.git$##')
fi
tag=${2:-latest}
if [ "$tag" = latest ]; then
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
code=$(curl -sL -o "$json" -w '%{http_code}' "$base/latest.json")
if [ "$code" != 200 ]; then
	echo "FAIL: latest.json answered HTTP $code"
	exit 1
fi

# One "platform url signature-length" line per platform.
platforms=$(node -e '
	const m = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
	console.error(`version ${m.version}`);
	for (const [name, p] of Object.entries(m.platforms ?? {}))
		console.log(`${name} ${p.url} ${(p.signature ?? "").length}`);
' "$json")
if [ -z "$platforms" ]; then
	echo "FAIL: latest.json lists no platforms"
	exit 1
fi
while read -r platform url siglen; do
	echo "--- $platform"
	check archive "$url"
	check .sig "$url.sig"
	if [ "$siglen" = 0 ]; then
		echo "  signature in latest.json: empty"
		failed=1
	fi
done <<EOF
$platforms
EOF

echo "--- downloads"
for name in MaxHQ_aarch64.dmg MaxHQ_x64.dmg \
	maxd-x86_64-unknown-linux-musl maxd-aarch64-unknown-linux-musl SHA256SUMS; do
	check "$name" "$base/$name"
done

if [ "$failed" = 0 ]; then
	echo "=== all checks passed ==="
else
	echo "=== some checks failed ==="
	exit 1
fi
