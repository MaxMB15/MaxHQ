// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// @ts-check
// The libraries a visual can name that the phone carries, by the file a page
// names after `maxhq:lib/`, and the file of each package that is that
// library, as the desktop's vite.config.ts lists them. The bundle carries
// each as its text, which the visual's page gets in place of its script.

/** What the app requires a library's text by: this, then its name. */
const VISUAL_LIBRARY_MODULE = "maxhq-visual-lib/";

/** @type {Readonly<Record<string, string>>} */
const VISUAL_LIBRARIES = {
	"chart.js": "chart.js/dist/chart.umd.min.js",
	"d3.js": "d3/dist/d3.min.js",
	"mermaid.js": "mermaid/dist/mermaid.min.js",
	"katex.js": "katex/dist/katex.min.js",
};

/**
 * Whether Metro's file is one of the libraries, by its path from any folder
 * the package manager put it in.
 *
 * @param {string} filename
 */
const isVisualLibrary = (filename) => {
	// Metro gives the path from the app's folder, which may start with node_modules.
	const at = `/${filename.split("\\").join("/")}`;
	return Object.values(VISUAL_LIBRARIES).some((file) => at.endsWith(`/node_modules/${file}`));
};

module.exports = { VISUAL_LIBRARY_MODULE, VISUAL_LIBRARIES, isVisualLibrary };
