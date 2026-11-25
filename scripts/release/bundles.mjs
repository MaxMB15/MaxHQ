// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Windows and Linux desktop builds a release carries, in one table the
// release scripts share: package-desktop.mjs names the installers it stages
// from it, sign-desktop signs each updater asset in it, latest-json.mjs turns
// each signature into a platform entry, and validate-release.sh checks the
// release has every asset.
//
// The assets carry no version, like the DMGs, so a download link to the
// latest release stays the same. The updater links name the release's tag.

// The Rust target, the updater's OS and CPU names, the CPU as the asset
// names have it (aarch64 or x64, as the Mac's do), and the installers built.
export const DESKTOP_TARGETS = [
	{ target: "x86_64-pc-windows-msvc", os: "windows", cpu: "x86_64", arch: "x64" },
	{ target: "aarch64-pc-windows-msvc", os: "windows", cpu: "aarch64", arch: "aarch64" },
	{ target: "x86_64-unknown-linux-gnu", os: "linux", cpu: "x86_64", arch: "x64" },
	{ target: "aarch64-unknown-linux-gnu", os: "linux", cpu: "aarch64", arch: "aarch64" },
];

// Each OS's Tauri bundle types, the first being what an app without a known
// bundle type updates from: an app run from an installer knows how it was
// installed, and the updater looks for `<os>-<cpu>-<bundle>` before
// `<os>-<cpu>`.
export const BUNDLES = { windows: ["nsis", "msi"], linux: ["appimage", "deb", "rpm"] };

// The asset name after MaxHQ_<arch>, per bundle type, which is also how
// Tauri's file for it ends.
export const SUFFIX = {
	nsis: "-setup.exe",
	msi: ".msi",
	appimage: ".AppImage",
	deb: ".deb",
	rpm: ".rpm",
};

// A maxd's release asset. The Windows ones have no .exe, like the key
// maxd-release.json gives them and the name the app's fleet copy looks for,
// so SHA256SUMS, the manifest and a folder of binaries all use one name.
export const maxdAsset = (target) => `maxd-${target}`;

export const assetName = (arch, bundle) => {
	if (!(bundle in SUFFIX)) throw new Error(`"${bundle}" isn't a desktop bundle type`);
	return `MaxHQ_${arch}${SUFFIX[bundle]}`;
};

// Every installer, in table order: its asset name, the target it's built
// for, its bundle type, and the updater platform keys that point at it.
export const desktopAssets = () =>
	DESKTOP_TARGETS.flatMap(({ target, os, cpu, arch }) =>
		BUNDLES[os].map((bundle, index) => ({
			asset: assetName(arch, bundle),
			target,
			bundle,
			platforms:
				index === 0
					? [`${os}-${cpu}`, `${os}-${cpu}-${bundle}`]
					: [`${os}-${cpu}-${bundle}`],
		}))
	);

export const targetInfo = (target) => {
	const info = DESKTOP_TARGETS.find((entry) => entry.target === target);
	if (!info)
		throw new Error(
			`"${target}" isn't a desktop target: ${DESKTOP_TARGETS.map((entry) => entry.target).join(", ")}`
		);
	return { ...info, bundles: BUNDLES[info.os] };
};

// Prints a list for the shell, one per line: `assets [<os>]` the installer
// names, `platforms [<os>]` the updater platform keys they fill, and `maxd`
// the Windows maxd asset names.
const main = () => {
	const [what, os] = process.argv.slice(2);
	const mine = (target) => !os || targetInfo(target).os === os;
	if (what === "assets") {
		for (const { asset, target } of desktopAssets()) if (mine(target)) console.log(asset);
	} else if (what === "platforms") {
		for (const { platforms, target } of desktopAssets())
			if (mine(target)) for (const platform of platforms) console.log(platform);
	} else if (what === "maxd") {
		for (const { target, os: system } of DESKTOP_TARGETS)
			if (system === "windows") console.log(maxdAsset(target));
	} else {
		console.error("usage: bundles.mjs assets|platforms [windows|linux] | maxd");
		process.exit(2);
	}
};

if (import.meta.main) main();
