// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { setThemeChoice } from "@/app/theme";
import { mockSource } from "@/data/mock-source";
import { clearMedia } from "@maxhq/shared/agent-view/media";
import { CODE_LIMIT, VIEWER_LIMIT } from "@maxhq/shared/agent-view/viewer-files";
import { SHOW_ANYWAY_SINCE } from "@maxhq/shared/bindings";
import { BINARY_SAMPLE, OUTSIDE_SAMPLE } from "@maxhq/shared/data/mock-files";
import type { DataSource } from "@maxhq/shared/data/source";
import { NOW } from "@maxhq/shared/test/build";

import { CodeLines } from "./code-file";
import { FilePaneFor } from "./file-pane";
import { clearReveals, revealLine } from "./reveal";

const AGENT = "agent-usage-popover";

const show = (path: string, source: DataSource = mockSource({ now: () => NOW, tick: false })) =>
	render(
		<ServicesProvider source={source}>
			<FilePaneFor agent={AGENT} path={path} />
		</ServicesProvider>
	);

// The sample source with every device's maxd on `protocol`, and its media
// fetches heard.
const speaking = (protocol: number | null) => {
	const sample = mockSource({ now: () => NOW, tick: false });
	const media = vi.fn<DataSource["media"]>(sample.media);
	const source: DataSource = {
		...sample,
		media,
		snapshot: async () => {
			const view = await sample.snapshot();
			return {
				...view,
				entities: view.entities.map((entity) =>
					entity.entity === "device_link" ? { ...entity, protocol } : entity
				),
			};
		},
	};
	return { source, media };
};

const framed = () =>
	waitFor(() => {
		const found = document.querySelector("iframe");
		if (!found) throw new Error("no frame yet");
		return found;
	});

afterEach(() => {
	clearMedia();
	clearReveals();
	setThemeChoice("system");
});

const RUST = "crates/usage/src/windows.rs";

// The text a copy of the whole file takes: the code without the gutter.
const codeText = (region: HTMLElement) =>
	[...region.querySelectorAll("pre")].map((pre) => pre.textContent).join("\n");

describe("a file pane", () => {
	it("draws a Markdown file itself, HTML and all", async () => {
		show("docs/usage-popover.md");
		expect(await screen.findByRole("heading", { name: "Usage popover" })).toBeInTheDocument();
		expect(screen.getByText("Why not keep the fixed rows?")).toBeInTheDocument();
		expect(document.querySelector("iframe")).toBeNull();
	});

	it("hands any other file to a frame that may run scripts and nothing else", async () => {
		show("design/meter-stand.glb");
		const frame = await framed();
		expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
		expect(frame.getAttribute("src")).toBe("/viewer.html");

		const into = frame.contentWindow!;
		const post = vi.spyOn(into, "postMessage").mockImplementation(() => {});
		// A message from anything but the frame is left alone.
		act(() => {
			window.dispatchEvent(
				new MessageEvent("message", {
					data: { type: "maxhq-viewer:ready" },
					source: window,
				})
			);
		});
		act(() => {
			window.dispatchEvent(
				new MessageEvent("message", { data: { type: "maxhq-viewer:ready" }, source: into })
			);
		});
		await waitFor(() => expect(post).toHaveBeenCalledTimes(1));
		const [message, target, moved] = post.mock.calls[0] as unknown as [
			{ type: string; name: string; extension: string; bytes: ArrayBuffer; theme: string },
			string,
			ArrayBuffer[],
		];
		expect(message).toMatchObject({
			type: "maxhq-viewer:open",
			name: "meter-stand.glb",
			extension: "glb",
			theme: "dark",
		});
		expect(new Uint32Array(message.bytes.slice(0, 4))[0]).toBe(0x46546c67);
		expect(target).toBe("*");
		expect(moved).toEqual([message.bytes]);

		expect(screen.getByText("Opening meter-stand.glb…")).toBeInTheDocument();
		act(() => {
			window.dispatchEvent(
				new MessageEvent("message", { data: { type: "maxhq-viewer:shown" }, source: into })
			);
		});
		expect(screen.queryByText("Opening meter-stand.glb…")).toBeNull();
	});

	it("says why when the frame couldn't draw the file", async () => {
		show("design/usage-samples.csv");
		const frame = await framed();
		act(() => {
			window.dispatchEvent(
				new MessageEvent("message", {
					data: { type: "maxhq-viewer:failed", message: "Bad table." },
					source: frame.contentWindow,
				})
			);
		});
		expect(screen.getByText("Couldn't show usage-samples.csv. Bad table.")).toBeInTheDocument();
	});

	it("opens the file again in a new frame when the theme changes", async () => {
		show("design/meter-stand.glb");
		const before = await framed();
		act(() => setThemeChoice("light"));
		const after = document.querySelector("iframe")!;
		expect(after).not.toBe(before);
		expect(screen.getByText("Opening meter-stand.glb…")).toBeInTheDocument();
		const post = vi.spyOn(after.contentWindow!, "postMessage").mockImplementation(() => {});
		act(() => {
			window.dispatchEvent(
				new MessageEvent("message", {
					data: { type: "maxhq-viewer:ready" },
					source: after.contentWindow,
				})
			);
		});
		await waitFor(() => expect(post).toHaveBeenCalledTimes(1));
		expect(post.mock.calls[0]![0]).toMatchObject({ theme: "light" });
	});

	it("offers Show anyway for a file outside the agent's folders, and shows it on a click", async () => {
		const { source, media } = speaking(SHOW_ANYWAY_SINCE);
		show(OUTSIDE_SAMPLE, source);
		expect(
			await screen.findByText("usage-handoff.md is outside the agent's folders.")
		).toBeInTheDocument();
		expect(media).toHaveBeenCalledTimes(1);
		expect(media.mock.calls[0]?.[3]).toBe(false);
		expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();

		fireEvent.click(screen.getByRole("button", { name: "Show anyway" }));
		expect(await screen.findByRole("heading", { name: "Handoff notes" })).toBeInTheDocument();
		expect(media).toHaveBeenCalledTimes(2);
		expect(media.mock.calls[1]?.slice(0, 4)).toEqual([
			AGENT,
			OUTSIDE_SAMPLE,
			VIEWER_LIMIT,
			true,
		]);
	});

	it("offers only Try again when a read fails another way", async () => {
		const { source, media } = speaking(SHOW_ANYWAY_SINCE);
		media.mockResolvedValue({
			status: "ok",
			data: { kind: "refused", outside: false, message: "no such file" },
		});
		show("docs/gone.md", source);
		expect(await screen.findByText("Couldn't read gone.md. No such file.")).toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Show anyway" })).toBeNull();

		media.mockResolvedValue({
			status: "error",
			error: { kind: "not_connected", device: "Studio" },
		});
		fireEvent.click(screen.getByRole("button", { name: "Try again" }));
		expect(
			await screen.findByText("Couldn't read gone.md. Studio isn't connected.")
		).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Show anyway" })).toBeNull();
	});

	it("hides Show anyway from a maxd that doesn't read it, and says to update it", async () => {
		for (const protocol of [SHOW_ANYWAY_SINCE - 1, null]) {
			const { source } = speaking(protocol);
			const { unmount } = show(OUTSIDE_SAMPLE, source);
			expect(
				await screen.findByText(
					/^usage-handoff\.md is outside the agent's folders\. Update maxd on .+ to show files outside the agent's folders\.$/
				)
			).toBeInTheDocument();
			expect(screen.queryByRole("button", { name: "Show anyway" })).toBeNull();
			unmount();
			clearMedia();
		}
	});

	it("draws code itself as numbered, read-only text", async () => {
		show(RUST);
		const region = await screen.findByRole("region", { name: "windows.rs, read-only" });
		expect(document.querySelector("iframe")).toBeNull();
		const code = codeText(region);
		expect(code).toContain("pub fn rows(windows: &[Window]) -> Vec<&Window> {");
		// The numbers come from CSS, so the text has none of them.
		const numbers = [...region.querySelectorAll("[data-n]")];
		expect(numbers.length).toBe(code.split("\n").length);
		expect(numbers[0]?.textContent).toBe("");
		expect(numbers[0]?.closest("[aria-hidden]")).not.toBeNull();
		expect(region.querySelector("textarea, [contenteditable]")).toBeNull();
	});

	it("asks for no more than the code limit and says when a file is past it", async () => {
		const { source, media } = speaking(SHOW_ANYWAY_SINCE);
		media.mockResolvedValue({ status: "ok", data: { kind: "too_large", size: 6_000_000 } });
		show("logs/huge.log", source);
		expect(
			await screen.findByText(
				"huge.log is 6.0 MB, too big to show here. Open it in an editor with the Open in button above."
			)
		).toBeInTheDocument();
		expect(media.mock.calls[0]?.[2]).toBe(CODE_LIMIT);
	});

	it("says a file with a zero byte is binary instead of drawing it", async () => {
		show(BINARY_SAMPLE);
		expect(
			await screen.findByText(
				"usage-poller.log looks like a binary file, so it isn't shown as text. Open it in an editor with the Open in button above."
			)
		).toBeInTheDocument();
		expect(screen.queryByRole("region", { name: /read-only/ })).toBeNull();
	});

	it("marks the line it was opened at, and the next one asked while open", async () => {
		revealLine(AGENT, RUST, 30);
		show(RUST);
		const region = await screen.findByRole("region", { name: "windows.rs, read-only" });
		expect(region.querySelector("[data-marked]")?.getAttribute("data-marked")).toBe("30");
		expect(region.querySelector('[data-n="30"]')?.className).toContain("text-foreground");

		act(() => revealLine(AGENT, RUST, 12));
		expect(region.querySelector("[data-marked]")?.getAttribute("data-marked")).toBe("12");
		// A line past the end marks the last one.
		act(() => revealLine(AGENT, RUST, 10_000));
		const last = region.querySelectorAll("[data-n]").length;
		expect(region.querySelector("[data-marked]")?.getAttribute("data-marked")).toBe(
			String(last)
		);
	});

	it("keeps a block's empty last line, so the code stays level with its numbers", () => {
		// Lines 200 and 201 are empty: the first block ends on one.
		const lines = Array.from({ length: 210 }, (_, i) =>
			i === 199 || i === 200 ? "" : `line ${i + 1}`
		);
		render(<CodeLines agent={AGENT} path="/tmp/example/gaps.txt" lines={lines} bytes={2000} />);
		const pres = [...document.querySelectorAll("pre")];
		expect(pres).toHaveLength(2);
		// A pre draws no line for a break it ends on, so the empty line has two.
		expect(pres[0]?.textContent?.endsWith("line 199\n\n")).toBe(true);
		expect(pres[1]?.textContent?.startsWith("\nline 202")).toBe(true);
		expect(pres[1]?.textContent?.endsWith("line 210")).toBe(true);
	});
});
