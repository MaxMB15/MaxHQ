// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { AgentRef, GitItemRef, NoteRef, QuoteRef } from "@maxhq/shared/pills/draft";
import { agent, NOW } from "@maxhq/shared/test/build";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { createPillRegistry, type FileRecord, PILL_KINDS, presentPill } from "./registry";
import { ShownPill } from "./shown";

const file = (over: Partial<FileRecord> = {}): FileRecord => ({
	name: "shot.png",
	kind: "image",
	bytes: 240 * 1024,
	mime: "image/png",
	path: "/Users/max/shot.png",
	image: null,
	fullImage: false,
	number: 1,
	progress: { state: "ready" },
	...over,
});

const quote: QuoteRef = {
	type: "quote",
	agent: "a2",
	agent_title: "Fix login",
	item: "item-1",
	text: "The cache misses on every cold start because the key holds the time",
	position: 0,
	source: "reply",
	from: null,
};

const agentRef: AgentRef = {
	type: "agent",
	agent: "a2",
	title: "Sidebar sort",
	vendor: "codex",
	device: "dev-mac",
	excerpt: null,
};

const pr: GitItemRef = {
	type: "git_item",
	repo: "owner/repo",
	number: 412,
	kind: "pull_request",
	title: "Cache the session key",
	state: "merged",
	url: "https://github.com/owner/repo/pull/412",
	head: "cache-key",
	base: "main",
	body: "",
	checked_at: NOW,
};

const show = (presentation: ReturnType<typeof presentPill>, selected = false) =>
	render(<ShownPill presentation={presentation} selected={selected} />);

const pill = () => document.querySelector<HTMLElement>(".pill")!;

describe("the pill registry", () => {
	it("needs a handler for every kind, and only one", () => {
		const handler = (kind: (typeof PILL_KINDS)[number]) => ({ kind, render: () => kind });
		expect(() =>
			createPillRegistry({
				handlers: [
					handler("attachment"),
					handler("quote"),
					handler("agent"),
					handler("note"),
					handler("visual"),
				],
				requiredKinds: PILL_KINDS,
				fallback: () => "none",
			})
		).toThrow("Missing pill handler: git_item");
		expect(() =>
			createPillRegistry({
				handlers: [handler("quote"), handler("quote")],
				fallback: () => "none",
			})
		).toThrow("Duplicate pill handler: quote");
	});

	it("falls back for a kind it doesn't know", () => {
		const registry = createPillRegistry({
			handlers: [{ kind: "quote", render: () => "quote" }],
			fallback: (kind) => `unknown ${kind}`,
		});
		expect(registry.render("diff", {}, null)).toBe("unknown diff");
		expect(registry.definition("quote").capabilities.details).toBe("popover");
	});
});

describe("a pill", () => {
	it("reads in full to a screen reader and cuts a long name in the middle", () => {
		show(
			presentPill(
				{
					kind: "attachment",
					file: file({
						name: "a-very-long-screenshot-name-from-the-meeting-today.png",
						progress: { state: "uploading", percent: 40 },
					}),
				},
				{ now: NOW }
			)
		);
		expect(pill()).toHaveAccessibleName(
			"Image 1, a-very-long-screenshot-name-from-the-meeting-today.png, 240 KB, uploading, 40 percent"
		);
		const label = pill().querySelector(".pill-label")!.textContent!;
		expect(Array.from(label)).toHaveLength(36);
		expect(label).toMatch(/^a-very-long.*….*today\.png$/);
	});

	it("fills as the file uploads, in the files' hue", () => {
		show(
			presentPill(
				{
					kind: "attachment",
					file: file({ progress: { state: "uploading", percent: 40 } }),
				},
				{ now: NOW }
			)
		);
		expect(pill()).toHaveAttribute("data-family", "file");
		expect(pill()).toHaveAttribute("data-state", "uploading");
		expect(pill().style.getPropertyValue("--pill-progress")).toBe("40%");
	});

	it("turns red with the reason when the file failed", () => {
		show(
			presentPill(
				{
					kind: "attachment",
					file: file({ progress: { state: "failed", error: "No such file" } }),
				},
				{ now: NOW }
			),
			true
		);
		expect(pill()).toHaveAttribute("data-state", "failed");
		expect(pill()).toHaveAccessibleName(/failed, No such file$/);
		expect(screen.getByRole("dialog")).toHaveTextContent("No such file");
	});

	it("shows a file or an agent that's gone as unresolved", () => {
		const { unmount } = show(
			presentPill({ kind: "attachment", file: file({ kind: "missing" }) }, { now: NOW })
		);
		expect(pill()).toHaveAttribute("data-state", "unresolved");
		unmount();

		show(
			presentPill(
				{ kind: "agent", ref: agentRef, agent: null, deviceName: "Mac" },
				{ now: NOW }
			)
		);
		expect(pill()).toHaveAttribute("data-state", "unresolved");
		expect(pill()).toHaveAccessibleName("Agent Sidebar sort, Codex, Mac, gone");
	});

	it("shows a note in yellow, with its title, project and last change", () => {
		const ref: NoteRef = {
			type: "note",
			project: "p1",
			note: "n1",
			title: "Old title",
			copy: null,
		};
		const { unmount } = show(
			presentPill(
				{
					kind: "note",
					ref,
					note: {
						note: "n1",
						title: "GTFS release steps",
						head: "h1",
						at: NOW - 5 * 60_000,
						by: null,
						created_on: "d1",
						deleted: false,
						size: 10,
					},
					projectName: "AtlasMaps",
					savedBy: "devbox",
					lines: <div>- Tag v2.4.0</div>,
				},
				{ now: NOW }
			),
			true
		);
		expect(pill()).toHaveAttribute("data-family", "note");
		expect(pill()).toHaveAttribute("data-state", "ready");
		expect(pill()).toHaveAccessibleName("Note GTFS release steps, AtlasMaps");
		const preview = screen.getByRole("dialog");
		expect(preview).toHaveTextContent("GTFS release steps");
		expect(preview).toHaveTextContent("Note in AtlasMaps");
		expect(preview).toHaveTextContent("Changed 5 min ago by devbox");
		expect(preview).toHaveTextContent("- Tag v2.4.0");
		unmount();

		// A deleted note keeps the title it was sent with.
		show(
			presentPill(
				{
					kind: "note",
					ref: { ...ref, title: "" },
					note: null,
					projectName: null,
					savedBy: null,
				},
				{ now: NOW }
			)
		);
		expect(pill()).toHaveAttribute("data-state", "unresolved");
		expect(pill().querySelector(".pill-label")).toHaveTextContent("Untitled");
		expect(pill()).toHaveAccessibleName("Note Untitled, gone");
	});

	it("names a live agent and opens its preview while selected", () => {
		show(
			presentPill(
				{
					kind: "agent",
					ref: agentRef,
					agent: agent({ id: "a2", title: "Sidebar sort", vendor: "codex" }),
					deviceName: "Mac",
				},
				{ now: NOW }
			),
			true
		);
		expect(pill()).toHaveAttribute("data-family", "agent");
		expect(pill()).toHaveAttribute("data-state", "ready");
		expect(screen.getByRole("dialog")).toHaveTextContent("Codex on Mac");
	});

	it("puts a quote's first words in quote marks", () => {
		show(presentPill({ kind: "quote", ref: quote }, { now: NOW }));
		expect(pill()).toHaveAttribute("data-family", "quote");
		expect(pill().textContent).toMatch(/^“The cache misses/);
		expect(pill()).toHaveAccessibleName(/^Quote from Fix login, /);
	});

	it("shades a pull request by its state, and previews its branches on hover", async () => {
		show(presentPill({ kind: "git_item", ref: pr }, { now: NOW }));
		expect(pill()).toHaveAttribute("data-family", "git-merged");
		expect(pill()).toHaveAccessibleName("Pull request 412, Cache the session key, merged");

		fireEvent.pointerEnter(pill(), { pointerType: "mouse" });
		fireEvent.mouseEnter(pill());
		fireEvent.mouseMove(pill());
		const preview = await waitFor(() => screen.getByRole("dialog"), { timeout: 2000 });
		expect(preview).toHaveTextContent("Pull request #412");
		expect(preview).toHaveTextContent("cache-key");
	});
});
