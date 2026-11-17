// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { agent, entities, link, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	backStep,
	bangStep,
	commandOfBlock,
	consoleCommand,
	homeShort,
	isShellLanguage,
	programOf,
	runPlace,
} from "./shell";

describe("shell mode steps", () => {
	it("goes from a message to a command to a private command, one ! at a time", () => {
		expect(bangStep("message")).toBe("shell");
		expect(bangStep("shell")).toBe("private");
		expect(bangStep("private")).toBeNull();
	});

	it("goes back one ! at a time with Backspace", () => {
		expect(backStep("private")).toBe("shell");
		expect(backStep("shell")).toBe("message");
		expect(backStep("message")).toBe("message");
	});
});

describe("shell code blocks", () => {
	it("knows the shell languages, in any case", () => {
		for (const lang of ["sh", "bash", "zsh", "shell", "console", "fish", "Bash", " zsh "])
			expect(isShellLanguage(lang)).toBe(true);
		for (const lang of ["ts", "python", "", null, undefined])
			expect(isShellLanguage(lang)).toBe(false);
	});

	it("takes a block as it is, without its last line breaks", () => {
		expect(commandOfBlock("rm -rf node_modules/.cache && pnpm dev\n", "sh")).toBe(
			"rm -rf node_modules/.cache && pnpm dev"
		);
		expect(commandOfBlock("cd app\npnpm install\n\n", "bash")).toBe("cd app\npnpm install");
		// A `$ ` outside a console block is part of the command.
		expect(commandOfBlock("$ pnpm test", "sh")).toBe("$ pnpm test");
	});

	it("keeps a console block's commands without their prompts or output", () => {
		const block = "$ pnpm test\n ✓ 3 passed\n$ git status --short\n M src/app.ts\n";
		expect(commandOfBlock(block, "console")).toBe("pnpm test\ngit status --short");
	});

	it("keeps a console command's continued lines", () => {
		expect(consoleCommand("$ docker run \\\n  --rm image\nok")).toBe(
			"docker run \\\n  --rm image"
		);
	});

	it("takes a console block with no prompt whole", () => {
		expect(consoleCommand("pnpm test\npnpm lint")).toBe("pnpm test\npnpm lint");
	});
});

describe("where a command runs", () => {
	it("writes the home folder as ~", () => {
		expect(homeShort("/Users/dev/code/example-app")).toBe("~/code/example-app");
		expect(homeShort("/home/dev/code/example-app")).toBe("~/code/example-app");
		expect(homeShort("/home/dev")).toBe("~");
		expect(homeShort("/root/app")).toBe("~/app");
		expect(homeShort("/srv/app")).toBe("/srv/app");
		expect(homeShort("/Users")).toBe("/Users");
	});

	it("names the agent's folder and device, and its container when it has one", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				agents: [
					agent({ id: "a1", workspace: "w1" }),
					agent({ id: "a2", workspace: "w2" }),
				],
				links: [link({ id: "mac", name: "Mac", device: "dev-mac" })],
				workspaces: [
					workspace({
						id: "w1",
						location: { type: "folder", path: "/Users/dev/code/example-app" },
					}),
					workspace({
						id: "w2",
						location: { type: "container", instance: "devbox", path: "/root/app" },
					}),
				],
			}),
		});
		expect(runPlace(view, "a1")).toEqual({
			device: "dev-mac",
			deviceName: "Mac",
			folder: "~/code/example-app",
			container: null,
		});
		expect(runPlace(view, "a2")).toMatchObject({ folder: "~/app", container: "devbox" });
		expect(runPlace(view, "gone")).toBeNull();
	});

	it("names the program a command starts", () => {
		expect(programOf("sudo systemctl restart nginx")).toBe("sudo");
		expect(programOf("  /usr/bin/sudo -v")).toBe("sudo");
		expect(programOf("")).toBe("");
	});
});
