// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ArtifactCall, ItemStatus } from "../bindings";
import { timelineItem } from "../timeline/build";
import { artifactLine, claudeLink } from "./artifact-line";
import { toolTarget } from "./format";
import { artifactWords, toolLines } from "./tool-line";

const LINK = "https://claude.ai/artifact/ExampleArtifactId0000001";

const call = (name: string, artifact: ArtifactCall, status: ItemStatus = "completed") =>
	timelineItem(
		{
			kind: "tool_call",
			server: null,
			name,
			title: "Artifact publish",
			input: null,
			output: null,
			artifact,
		},
		{ id: "i", status }
	);

const words = (item: ReturnType<typeof call>) => artifactWords(item.status, artifactLine(item)!);

describe("claudeLink", () => {
	it("keeps an https link on claude.ai", () => {
		expect(claudeLink(LINK)).toBe(LINK);
		const query = "https://claude.ai/artifact/ExampleArtifactId0000002?tab=data%20view#top";
		expect(claudeLink(query)).toBe(query);
	});

	it("drops any other link", () => {
		for (const text of [
			"http://claude.ai/artifact/ExampleArtifactId0000001",
			"https://claude.ai.example.com/artifact/x",
			"https://evil@claude.ai/artifact/x",
			"https://claude.ai:8443/artifact/x",
			"https://claude.ai\\@example.com/x",
			"https://claude.ai",
			"https://CLAUDE.AI/artifact/x",
			`${LINK} more`,
			`${LINK}"><b>`,
			`${LINK}\nhttps://example.com`,
			`${LINK}\n`,
			"javascript:alert(1)//https://claude.ai/",
			"https://example.com/?u=https://claude.ai/",
			"",
			null,
		])
			expect(claudeLink(text)).toBeNull();
	});
});

describe("artifactLine", () => {
	it("reads a publish by its status, with its version and link", () => {
		const published = call("Artifact", {
			action: "publish",
			title: "Weekly report",
			url: LINK,
			detail: "report.html",
			version: "2",
			owned: true,
		});
		expect(artifactLine(published)).toEqual({
			tool: "page",
			verbs: expect.anything(),
			target: "Weekly report",
			note: "report.html, v2",
			url: LINK,
		});
		expect(words(published)).toBe("Published Weekly report");
		expect(words({ ...published, status: "running" })).toBe("Publishing Weekly report");
		expect(words({ ...published, status: "declined" })).toBe("Didn't publish Weekly report");
	});

	it("says when the page is someone else's", () => {
		const read = call("Artifact", { action: "read", title: "Launch checklist", owned: false });
		expect(artifactLine(read)?.note).toBe("someone else's");
		expect(words({ ...read, status: "waiting" })).toBe("Wants to read Launch checklist");
	});

	it("falls back to the link, then what the call named, then a page", () => {
		expect(artifactLine(call("Artifact", { action: "open", url: LINK }))?.target).toBe(LINK);
		const named = artifactLine(call("Artifact", { action: "delete", detail: "old.html" }));
		expect([named?.target, named?.note]).toEqual(["old.html", null]);
		expect(words(call("Artifact", { action: "pin" }))).toBe("Pinned a page");
	});

	it("never keeps a link that isn't on claude.ai", () => {
		const line = artifactLine(
			call("Artifact", { action: "read", title: "Report", url: "https://example.com/x" })
		);
		expect(line?.url).toBeNull();
	});

	it("lets a verb that says it all stand alone", () => {
		expect(words(call("Artifact", { action: "list" }))).toBe("Listed your claude.ai pages");
		const started = call("Artifact", { action: "quickstart", detail: "slides" });
		expect([words(started), artifactLine(started)?.note]).toEqual(["Started a page", "slides"]);
	});

	it("reads data, comment and check calls", () => {
		const saved = call("ArtifactData", {
			action: "set",
			title: "Launch checklist",
			detail: "tasks",
		});
		expect([artifactLine(saved)?.tool, words(saved), artifactLine(saved)?.note]).toEqual([
			"data",
			"Saved data to Launch checklist",
			"tasks",
		]);
		expect(words(call("ArtifactData", { action: "query", title: "Launch checklist" }))).toBe(
			"Read data from Launch checklist"
		);
		expect(words(call("ArtifactComments", { action: "reply", title: "Weekly report" }))).toBe(
			"Replied to a comment on Weekly report"
		);
		expect(words(call("ArtifactCheck", { action: "verify", title: "Weekly report" }))).toBe(
			"Checked Weekly report"
		);
	});

	it("reads an action it doesn't know as a use of the tool", () => {
		expect(words(call("Artifact", { action: "archive", title: "Weekly report" }))).toBe(
			"Used Artifacts on Weekly report"
		);
	});

	it("keeps the page's name on one line", () => {
		expect(
			artifactLine(call("Artifact", { action: "read", title: "Weekly\n\u001b[31mreport" }))
				?.target
		).toBe("Weekly [31mreport");
	});

	it("leaves other tool calls alone", () => {
		const plain = timelineItem(
			{
				kind: "tool_call",
				server: null,
				name: "Artifact",
				title: "Artifact publish",
				input: null,
				output: null,
			},
			{ id: "p" }
		);
		expect(artifactLine(plain)).toBeUndefined();
		expect(toolTarget(plain)).toBe("Artifact");
	});
});

describe("an artifact call in text", () => {
	const names = { agent: () => undefined, device: () => "devbox" };

	it("reads as its verb, page and link, in place of the item's own title", () => {
		const published = call("Artifact", {
			action: "publish",
			title: "Weekly report",
			url: LINK,
			version: "3",
		});
		expect(toolLines(published, names)).toEqual([`Published Weekly report, v3 <${LINK}>`]);
		expect(toolLines({ ...published, status: "failed" }, names)).toEqual([
			`Published Weekly report, v3 <${LINK}> (failed)`,
		]);
		expect(toolTarget(published)).toBe("Weekly report");
	});
});
