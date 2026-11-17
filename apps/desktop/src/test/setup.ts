// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import "@testing-library/jest-dom/vitest";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { cleanup, configure } from "@testing-library/react";
import { afterEach } from "vitest";

// A dialog or menu can take more than a second to show on a busy machine,
// which fails the commit hook for nothing. A real miss still fails.
configure({ asyncUtilTimeout: 5_000 });

// Vitest runs without globals, so Testing Library can't register its own
// cleanup. Unmount what each test rendered, and forget what it stored, such
// as a pane layout or a composer's draft.
afterEach(() => {
	cleanup();
	localStorage.clear();
	clearKept();
});

// jsdom has no ResizeObserver, and the pane layout watches its size with
// one. Nothing in jsdom has a size, so one that never reports is enough.
if (!("ResizeObserver" in globalThis)) {
	globalThis.ResizeObserver = class {
		observe() {}
		unobserve() {}
		disconnect() {}
	};
}

// With a ResizeObserver, Base UI's scroll areas also wait on animations,
// which jsdom can't list. It runs none, so an empty list is right.
if (typeof Element.prototype.getAnimations !== "function") {
	Element.prototype.getAnimations = () => [];
}

// jsdom has no layout, so a Range has no client rects at all. TipTap focuses
// an editor a frame later and scrolls the caret into view then, which asks
// ProseMirror for the caret's rects. That frame can land after a test ends,
// so give every Range the empty rects jsdom gives an element. A test that
// needs real positions sets its own.
if (typeof Range.prototype.getClientRects !== "function") {
	Range.prototype.getClientRects = () =>
		Object.assign([], { item: () => null }) as unknown as DOMRectList;
}
if (typeof Range.prototype.getBoundingClientRect !== "function") {
	Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
}
