// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Expo's defaults already watch the whole workspace, so the shared package
// loads from its source like any file of the app. A visual library, which
// the app requires as `maxhq-visual-lib/chart.js`, resolves to its package's
// file, and the transformer makes that file its text.

const path = require("node:path");

const { getDefaultConfig } = require("expo/metro-config");

const { VISUAL_LIBRARIES, VISUAL_LIBRARY_MODULE } = require("./visual-libraries");

const config = getDefaultConfig(__dirname);

config.transformer.babelTransformerPath = require.resolve("./visual-transformer");

config.resolver.resolveRequest = (context, moduleName, platform) => {
	const file = moduleName.startsWith(VISUAL_LIBRARY_MODULE)
		? VISUAL_LIBRARIES[moduleName.slice(VISUAL_LIBRARY_MODULE.length)]
		: undefined;
	return file
		? { type: "sourceFile", filePath: path.join(__dirname, "node_modules", file) }
		: context.resolveRequest(context, moduleName, platform);
};

module.exports = config;
