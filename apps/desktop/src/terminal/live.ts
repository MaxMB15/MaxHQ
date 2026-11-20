// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The window's terminal registry, over its source's terminal bytes, and the
// size of one cell of the terminal font.

import type { Services } from "@/app/services";
import { noticesOf } from "@/sidebar/notices";
import { terminalById } from "@maxhq/shared/terminal/select";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";

import { createRegistry, type Registry } from "./registry";
import { FONT_SIZE, LINE_HEIGHT, monoFamily, xtermView } from "./term-view-xterm";

const registries = new WeakMap<Services, Registry | null>();

/**
 * The window's live terminals, or null when its source carries no terminal
 * bytes.
 */
export const registryOf = (services: Services): Registry | null => {
	if (registries.has(services)) return registries.get(services) ?? null;
	const stream = services.source.terminalStream;
	const entity = (terminal: string) => terminalById(services.view.getState().view, terminal);
	const registry = stream
		? createRegistry({
				stream,
				makeView: xtermView,
				sizeOf: (terminal) => {
					const found = entity(terminal);
					return found ? { cols: found.cols, rows: found.rows } : null;
				},
				exists: (terminal) => entity(terminal) !== undefined,
				marksOf: (terminal) => entity(terminal)?.marks === true,
				onFailure: (_terminal, message) => showFailure(noticesOf(services), message),
			})
		: null;
	registries.set(services, registry);
	return registry;
};

let cell: { readonly width: number; readonly height: number } | null = null;

/** One cell of the terminal font, in CSS pixels, measured once. */
export const cellSize = (): { readonly width: number; readonly height: number } => {
	if (cell) return cell;
	const probe = document.createElement("span");
	probe.style.cssText = `position:absolute;visibility:hidden;white-space:pre;font-family:${monoFamily()};font-size:${FONT_SIZE}px;line-height:normal`;
	probe.textContent = "W".repeat(10);
	document.body.append(probe);
	const box = probe.getBoundingClientRect();
	probe.remove();
	// Nothing has a size without layout, as in tests, so fall back to the usual cell.
	if (box.width <= 0) return { width: 8, height: 17 };
	// The terminal spaces lines as its line height says, in whole pixels.
	cell = { width: box.width / 10, height: Math.floor(box.height * LINE_HEIGHT) };
	return cell;
};
