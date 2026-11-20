// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ModelFile } from "../bindings";

import { GIB } from "./fit";
import {
	coldTurnText,
	fileStateText,
	formatFileSize,
	gpuLimitText,
	loadText,
	localAgentLabel,
	notEncrypted,
	runtimeSupportText,
	scoreSourceText,
	serverProblemText,
	urlParts,
} from "./wording";

const file = (state: ModelFile["state"], done = 5e9, total = 20e9): ModelFile => ({
	model: "m",
	file: "m.gguf",
	path: "/models/m.gguf",
	bytes_done: done,
	bytes_total: total,
	state,
});

describe("server URLs", () => {
	it("reads the scheme, host and port", () => {
		expect(urlParts("http://gpu-box.example.ts.net:11434/v1")).toEqual({
			scheme: "http",
			host: "gpu-box.example.ts.net",
			port: "11434",
		});
		expect(urlParts(" HTTPS://user:secret@Devbox ")).toEqual({
			scheme: "https",
			host: "devbox",
			port: "",
		});
		expect(urlParts("http://[::1]:8080")).toMatchObject({ host: "[::1]", port: "8080" });
		expect(urlParts("devbox:11434")).toBeNull();
		expect(urlParts("")).toBeNull();
	});

	it("calls plain HTTP to another machine not encrypted, and nothing else", () => {
		const is = (base_url: string | null) => notEncrypted({ base_url });
		expect(is("http://gpu-box.example.ts.net:11434")).toBe(true);
		expect(is("http://192.168.1.20:8080")).toBe(true);
		expect(is("https://gpu-box.example.ts.net")).toBe(false);
		expect(is("http://127.0.0.1:8080")).toBe(false);
		expect(is("http://localhost:1234")).toBe(false);
		expect(is("http://[::1]:8080")).toBe(false);
		expect(is(null)).toBe(false);
	});

	it("says why a server can't run a Local agent", () => {
		const probe = { at: 0, apis: { messages: true }, context: null, error: null };
		expect(serverProblemText({ probe: null })).toBeNull();
		expect(serverProblemText({ probe })).toBeNull();
		expect(serverProblemText({ probe: { ...probe, apis: { chat: true } } })).toMatch(
			/Anthropic Messages API/
		);
		expect(serverProblemText({ probe: { ...probe, error: "Nothing answered" } })).toBe(
			"Nothing answered"
		);
	});
});

describe("model wording", () => {
	it("names a local agent Local with its model", () => {
		expect(localAgentLabel("Qwen3.6-35B-A3B")).toBe("Local · Qwen3.6-35B-A3B");
		expect(localAgentLabel(null)).toBe("Local");
	});

	it("says where a download and a load stand", () => {
		expect(fileStateText(file({ state: "downloading", phase: "downloading" }))).toBe(
			"Downloading, 25%"
		);
		expect(fileStateText(file({ state: "downloading", phase: "verifying" }))).toBe(
			"Checking the download"
		);
		expect(fileStateText(file({ state: "paused" }))).toBe("Paused, 25%");
		expect(fileStateText(file({ state: "ready" }))).toBe("Downloaded");
		expect(fileStateText(file({ state: "failed", reason: "disk full" }))).toBe(
			"Download failed: disk full"
		);
		expect(loadText(null)).toBeNull();
		expect(loadText({ state: "loading" })).toBe("Loading model");
		expect(loadText({ state: "failed", reason: "out of memory" })).toBe(
			"Couldn't load: out of memory"
		);
	});

	it("gives sizes, turn times and sources in plain words", () => {
		expect(formatFileSize(22_400_000_000)).toBe("22.4 GB");
		expect(formatFileSize(640_000_000)).toBe("640 MB");
		expect(coldTurnText({ low: 18.2, high: 33.6 })).toBe("First turn about 18-34 s");
		expect(coldTurnText({ low: 150, high: 400 })).toBe("First turn about 3-7 min");
		expect(scoreSourceText({ source: "vendor", date: "2026-04-16" })).toBe(
			"Reported by the vendor, 2026-04-16"
		);
		expect(scoreSourceText({ source: "independent", date: "2026-05-02" })).toBe(
			"Measured independently, 2026-05-02"
		);
	});

	it("explains the GPU limit and leaves it alone", () => {
		const hardware = { chip: "Apple M4 Max", gpu_cores: 32, ram: 36 * GIB };
		expect(gpuLimitText({ ...hardware, gpu: { budget: 27 * GIB, source: "metal" } })).toMatch(
			/^macOS lets the GPU use about 27 GB of this Mac's 36 GB.*doesn't raise the limit/
		);
		expect(
			gpuLimitText({ ...hardware, gpu: { budget: 32 * GIB, source: "wired_limit" } })
		).toMatch(/iogpu\.wired_limit_mb\. MaxHQ leaves it as it is\.$/);
		expect(gpuLimitText({ ...hardware, gpu: null })).toBeNull();
		expect(runtimeSupportText("managed")).toBeNull();
		expect(runtimeSupportText("no_gpu")).toMatch(/^Too slow for agents/);
	});
});
