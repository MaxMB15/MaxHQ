// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { isPlainHost, MOST_HOSTS } from "../../packages/shared/src/visuals/frame";

// Set by `tauri dev` when the webview runs on another device, such as a phone.
const host = process.env.TAURI_DEV_HOST;

// What the viewer frame may load while Vite serves it. The built frame gets
// its policy from the app's viewer scheme instead (crates/app/src/viewer.rs).
// Vite's own scripts are inline and its client wants a socket, and neither
// reaches past this machine.
const DEV_FRAME_POLICY = [
	"default-src 'none'",
	"script-src http://localhost:* http://127.0.0.1:* 'unsafe-inline' 'wasm-unsafe-eval'",
	"style-src http://localhost:* http://127.0.0.1:* 'unsafe-inline'",
	"img-src blob: data:",
	"media-src blob: data:",
	"font-src data: http://localhost:* http://127.0.0.1:*",
	"connect-src blob: data:",
	"worker-src blob:",
	"frame-src 'none'",
	"form-action 'none'",
	"base-uri 'none'",
].join("; ");

// The viewer frame runs on an opaque origin, so the module scripts it loads
// from Vite are cross-origin requests from origin "null". Vite answers only
// localhost origins, so in development this lets the frame's own requests
// through: an origin of "null" whose referrer is a page of this server. A
// sandboxed frame on another site sends that site as its referrer.
const viewerFrame = (): Plugin => ({
	name: "maxhq-viewer-frame",
	apply: "serve",
	configureServer: (server) => {
		server.middlewares.use((req, res, next) => {
			const { origin, referer, host: served } = req.headers;
			if (origin === "null" && referer && URL.parse(referer)?.host === served)
				res.setHeader("Access-Control-Allow-Origin", "null");
			next();
		});
	},
	transformIndexHtml: {
		order: "pre",
		handler: (html, { path: page }) =>
			page === "/viewer.html"
				? html.replace(
						"<head>",
						`<head>\n\t\t<meta http-equiv="Content-Security-Policy" content="${DEV_FRAME_POLICY}" />`
					)
				: html,
	},
});

// The libraries a visual can load as `maxhq:lib/<name>`, each a file of a
// package the app already has. The frame serves them at `/visual-lib/`, as
// do the app's scheme (crates/app/src/viewer.rs, whose list matches this
// one) and, while developing, this server. Three's module imports its core
// beside it, so that goes too.
const LIBRARIES: Record<string, string> = {
	"chart.js": "chart.js/dist/chart.umd.min.js",
	"d3.js": "d3/dist/d3.min.js",
	"three.js": "three/build/three.module.js",
	"three.core.js": "three/build/three.core.js",
	"mermaid.js": "mermaid/dist/mermaid.min.js",
	"katex.js": "katex/dist/katex.min.js",
	"katex.css": "katex/dist/katex.min.css",
};
const KATEX_FONTS = "katex/dist/fonts";
const LIBRARY_PATH = "/visual-lib/";

const modules = path.resolve(import.meta.dirname, "node_modules");

// KaTeX's styles name each font in three formats, and every browser the app
// runs in takes the first.
const katexFonts = () =>
	readdirSync(path.join(modules, KATEX_FONTS)).filter((name) => name.endsWith(".woff2"));

// The file `/visual-lib/<name>` serves, if it's one.
const libraryFile = (name: string): string | null => {
	const file = LIBRARIES[name];
	if (file) return path.join(modules, file);
	const font = name.startsWith("fonts/") ? name.slice("fonts/".length) : null;
	return font && katexFonts().includes(font) ? path.join(modules, KATEX_FONTS, font) : null;
};

const TYPES: Record<string, string> = {
	js: "text/javascript",
	css: "text/css",
	woff2: "font/woff2",
};

// What a visual's frame may load while Vite serves it, as the app's scheme
// allows in the built app, with this server for the app's own. The hosts
// Load this time let through come in the page's query.
const devVisualPolicy = (hosts: string[]) => {
	const own = ["http://localhost:*", "http://127.0.0.1:*", ...hosts.map((h) => `https://${h}`)];
	const from = own.join(" ");
	return [
		"default-src 'none'",
		`script-src ${from} 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' blob: data:`,
		`style-src ${from} 'unsafe-inline' blob: data:`,
		`img-src ${from} blob: data:`,
		`font-src ${from} data:`,
		"media-src blob: data:",
		"connect-src 'none'",
		"worker-src blob:",
		"frame-src 'none'",
		"form-action 'none'",
		"base-uri 'none'",
	].join("; ");
};

const hostsOf = (url: string | undefined): string[] =>
	(new URLSearchParams(url?.split("?")[1] ?? "").get("hosts") ?? "")
		.split(",")
		.filter(isPlainHost)
		.slice(0, MOST_HOSTS);

// The visual frame's page and the libraries it serves. The build copies each
// library beside the app. While developing, this server serves them, and the
// page leaves out Vite's client, whose socket the policy refuses.
const visualFrame = (): Plugin => ({
	name: "maxhq-visual-frame",
	configureServer: (server) => {
		server.middlewares.use((req, res, next) => {
			const at = req.url?.split("?")[0] ?? "";
			const file = at.startsWith(LIBRARY_PATH)
				? libraryFile(at.slice(LIBRARY_PATH.length))
				: null;
			if (!file) return next();
			res.setHeader(
				"Content-Type",
				TYPES[path.extname(file).slice(1)] ?? "application/octet-stream"
			);
			res.setHeader("Access-Control-Allow-Origin", "*");
			res.end(readFileSync(file));
		});
	},
	generateBundle() {
		const names = [...Object.keys(LIBRARIES), ...katexFonts().map((font) => `fonts/${font}`)];
		for (const name of names)
			this.emitFile({
				type: "asset",
				fileName: `visual-lib/${name}`,
				source: readFileSync(libraryFile(name)!),
			});
	},
	transformIndexHtml: {
		order: "post",
		handler: (html, { path: page, originalUrl, server }) =>
			page === "/visual.html" && server
				? html
						.replace(/<script type="module" src="\/@vite\/client"><\/script>\s*/, "")
						.replace(
							"<head>",
							`<head>\n\t\t<meta http-equiv="Content-Security-Policy" content="${devVisualPolicy(hostsOf(originalUrl))}" />\n\t\t<meta http-equiv="x-dns-prefetch-control" content="off" />`
						)
				: html,
	},
});

export default defineConfig({
	plugins: [react(), tailwindcss(), viewerFrame(), visualFrame()],
	resolve: {
		alias: {
			"@": path.resolve(import.meta.dirname, "./src"),
			// The 3D renderer's STEP reader, whose license the app can't ship.
			"@file-viewer/geometry-engine": path.resolve(
				import.meta.dirname,
				"./src/viewer/geometry-stub.ts"
			),
		},
	},
	build: {
		rollupOptions: {
			input: {
				main: path.resolve(import.meta.dirname, "index.html"),
				viewer: path.resolve(import.meta.dirname, "viewer.html"),
				visual: path.resolve(import.meta.dirname, "visual.html"),
			},
		},
	},
	// Tauri prints its own errors, and clearing the screen would hide them.
	clearScreen: false,
	server: {
		// tauri.conf.json points devUrl here, so the port can't move.
		port: 1420,
		strictPort: true,
		host: host || false,
		hmr: host
			? {
					protocol: "ws",
					host,
					port: 1421,
				}
			: undefined,
		watch: {
			// Cargo rebuilds the Rust side. Vite reloading on it would only
			// restart the page for nothing.
			ignored: ["**/src-tauri/**"],
		},
	},
});
