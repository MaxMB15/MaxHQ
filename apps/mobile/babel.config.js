// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

module.exports = function (api) {
	api.cache(true);
	return { presets: ["babel-preset-expo"] };
};
