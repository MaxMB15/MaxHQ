// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Puts the saved theme on the page before the first paint, so a light
// window never opens dark. It runs before the app's modules load, so it
// can't share their code: src/app/theme.ts reads the same key the same way,
// and a test there checks the two agree. The content policy allows no
// inline script, so this is a file of its own.

(() => {
	let choice = null;
	try {
		choice = localStorage.getItem("maxhq.theme");
	} catch {
		// Storage that throws leaves the theme on System.
	}
	const systemDark =
		typeof matchMedia !== "function" || matchMedia("(prefers-color-scheme: dark)").matches;
	const dark = choice === "dark" || (choice !== "light" && systemDark);
	const root = document.documentElement;
	root.dataset.theme = dark ? "dark" : "light";
	root.classList.toggle("dark", dark);
})();
