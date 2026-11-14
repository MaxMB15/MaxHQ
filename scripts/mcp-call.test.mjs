// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { test } from "node:test";
import { exchange } from "./mcp-call.mjs";

// A stand-in server: it answers initialize, and answers the next request
// with every method it read, so the test sees the handshake's order.
const server = `
const seen = [];
require("node:readline").createInterface({ input: process.stdin }).on("line", (line) => {
	const message = JSON.parse(line);
	seen.push(message.method);
	if (message.id === 1) console.log(JSON.stringify({ jsonrpc: "2.0", id: 1, result: {} }));
	else if (message.params?.name === "nope")
		console.log(JSON.stringify({ jsonrpc: "2.0", id: message.id, error: { message: "No tool nope." } }));
	else if (message.id !== undefined)
		console.log(JSON.stringify({ jsonrpc: "2.0", id: message.id, result: { seen, params: message.params } }));
});
`;

const run = (method, params) => exchange(process.execPath, ["-e", server], method, params);

test("initializes, then sends the request", async () => {
	const result = await run("tools/call", { name: "list_agents", arguments: {} });
	assert.deepEqual(result.seen, ["initialize", "notifications/initialized", "tools/call"]);
	assert.deepEqual(result.params, { name: "list_agents", arguments: {} });
});

test("an error answer rejects", async () => {
	await assert.rejects(run("tools/call", { name: "nope" }), /No tool nope/);
});

test("a server that exits first rejects", async () => {
	await assert.rejects(
		exchange(process.execPath, ["-e", "process.exit(3)"], "tools/list", {}),
		/exited with 3/
	);
});
