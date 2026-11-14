// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Drives `maxd mcp` the way a bot outside MaxHQ would: it starts the server
// with an access key file, does the MCP handshake, sends one request and
// prints the answer. It stands in for the Slack bot in the agent tools check.
//
//   node scripts/mcp-call.mjs <key file>                     lists the tools
//   node scripts/mcp-call.mjs <key file> <tool> ['<json>']   calls one
//
// MAXD names the maxd binary, which is `maxd` on the PATH by default.

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const MCP_VERSION = "2025-06-18";

/**
 * Starts the stdio MCP server `command`, initializes it, then sends `method`
 * with `params`. Resolves with the answer's result and rejects with its error.
 */
export const exchange = (command, args, method, params) =>
	new Promise((resolve, reject) => {
		const server = spawn(command, args, { stdio: ["pipe", "pipe", "inherit"] });
		const send = (message) =>
			server.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`);
		server.on("error", reject);
		server.on("exit", (code) => reject(new Error(`the server exited with ${code} first`)));
		createInterface({ input: server.stdout }).on("line", (line) => {
			let message;
			try {
				message = JSON.parse(line);
			} catch {
				return;
			}
			if (message.id === 1) {
				send({ method: "notifications/initialized" });
				send({ id: 2, method, params });
			} else if (message.id === 2) {
				server.stdin.end();
				if (message.error) reject(new Error(message.error.message));
				else resolve(message.result);
			}
		});
		send({
			id: 1,
			method: "initialize",
			params: {
				protocolVersion: MCP_VERSION,
				capabilities: {},
				clientInfo: { name: "mcp-call", version: "1" },
			},
		});
	});

const main = async () => {
	const [keyFile, tool, json] = process.argv.slice(2);
	if (!keyFile) {
		console.error("usage: node scripts/mcp-call.mjs <key file> [<tool> ['<json arguments>']]");
		process.exit(2);
	}
	const command = process.env.MAXD ?? "maxd";
	const args = ["mcp", "--key-file", keyFile];
	if (!tool) {
		const { tools } = await exchange(command, args, "tools/list", {});
		for (const { name, description } of tools) console.log(`${name}: ${description}`);
		return;
	}
	const result = await exchange(command, args, "tools/call", {
		name: tool,
		arguments: json ? JSON.parse(json) : {},
	});
	for (const { text } of result.content) console.log(text);
	if (result.isError) process.exit(1);
};

// Run as a script, not when the tests import exchange.
if (import.meta.main)
	main().catch((err) => {
		console.error(`mcp-call: ${err.message}`);
		process.exit(1);
	});
