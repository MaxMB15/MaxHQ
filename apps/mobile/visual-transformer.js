// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// @ts-check
// Expo's Babel transformer, except that a visual library becomes a module
// whose export is the library's text, so the bundle carries it to drop into
// a visual's page and never runs it in the app.

const path = require("node:path");

const { isVisualLibrary, VISUAL_LIBRARIES } = require("./visual-libraries");

const expo = path.dirname(require.resolve("expo/package.json"));
const upstream = require(
	require.resolve("@expo/metro-config/build/babel-transformer", { paths: [expo] })
);

/** @param {{ src: string; filename: string }} args */
const transform = (args) =>
	upstream.transform(
		isVisualLibrary(args.filename)
			? { ...args, src: `module.exports = ${JSON.stringify(args.src)};` }
			: args
	);

module.exports = {
	...upstream,
	transform,
	// A change to the list makes Metro read the libraries again.
	getCacheKey: () => `${upstream.getCacheKey?.() ?? ""}:${JSON.stringify(VISUAL_LIBRARIES)}`,
};
