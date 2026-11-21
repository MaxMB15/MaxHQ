// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Shell } from "@/App";
import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { openLogin } from "@/test/settings";
import type { CursorRuleCall, CursorRuleList, DeviceId } from "@maxhq/shared/bindings";
import type { Result } from "@maxhq/shared/data/request-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const RISK =
	"Cursor runs commands that start with these words without asking MaxHQ, in every project on this device. Always allow in an earlier MaxHQ saved them; MaxHQ now keeps those rules itself.";

type Answer = (device: DeviceId, call: CursorRuleCall) => Promise<Result<CursorRuleList>>;

/** The window over the sample source, with Cursor's rules from `answer` when given. */
const renderShell = async (answer?: Answer) => {
	const sample = mockSource({ now: () => NOW, tick: false });
	const source = answer ? { ...sample, cursorRules: answer } : sample;
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={createUiStore()}>
				<Shell />
			</ServicesProvider>
		</FixedNow>
	);
	await screen.findAllByRole("button", { name: /devbox/ });
	return source;
};

/** The card on the Cursor page, and the Mac's part of it. */
const openCard = async () => {
	const settings = await openLogin("Cursor");
	const card = within(settings).getByRole("region", { name: "Shell rules Cursor saved" });
	return { card, mac: within(card).getByRole("region", { name: "On Mac" }) };
};

const listed = (rules: string[]): Result<CursorRuleList> => ({
	status: "ok",
	data: { status: "listed", rules: { path: "~/.cursor/cli-config.json", rules } },
});

describe("Shell rules Cursor saved", () => {
	it("lists the Mac's rules with what they let through, and removes one", async () => {
		const source = await renderShell();
		const ask = vi.spyOn(source, "cursorRules");
		const { card, mac } = await openCard();

		expect(await within(mac).findByText("git status")).toBeInTheDocument();
		expect(within(mac).getByText("pnpm test")).toBeInTheDocument();
		expect(within(mac).getByText("cargo build")).toBeInTheDocument();
		expect(within(mac).getByText(RISK)).toBeInTheDocument();
		expect(within(mac).getByText("~/.cursor/cli-config.json")).toBeInTheDocument();
		// devbox has no Cursor CLI, so it has no part.
		expect(within(card).queryByRole("region", { name: "On devbox" })).toBeNull();

		fireEvent.click(within(mac).getByRole("button", { name: "Remove pnpm test on Mac" }));
		expect(
			await within(mac).findByText(
				"Removed. Cursor agents running now keep it until they start again."
			)
		).toBeInTheDocument();
		expect(within(mac).queryByText("pnpm test")).toBeNull();
		expect(within(mac).getByText("git status")).toBeInTheDocument();
		expect(ask).toHaveBeenCalledWith("dev-mac", { kind: "list" });
		expect(ask).toHaveBeenCalledWith("dev-mac", { kind: "remove", rule: "Shell(pnpm test)" });
	});

	it("keeps the list when a removal fails, and says why", async () => {
		await renderShell(async (_, call) =>
			call.kind === "list"
				? listed(["Shell(ls)"])
				: {
						status: "error",
						error: { kind: "failed", message: "Cursor's settings changed shape" },
					}
		);
		const { mac } = await openCard();

		fireEvent.click(await within(mac).findByRole("button", { name: "Remove ls on Mac" }));
		expect(await within(mac).findByRole("status")).toHaveTextContent(
			"Cursor's settings changed shape"
		);
		expect(within(mac).getByText("ls")).toBeInTheDocument();
	});

	it("says so quietly when Cursor saved none", async () => {
		await renderShell(async () => listed([]));
		const { mac } = await openCard();

		expect(
			await within(mac).findByText("Cursor saved no shell rules here.")
		).toBeInTheDocument();
		expect(within(mac).queryByText(RISK)).toBeNull();
		expect(within(mac).queryByRole("button", { name: /^Remove/ })).toBeNull();
	});

	it("asks for a newer maxd on a device too old to read them", async () => {
		await renderShell(async () => ({ status: "ok", data: { status: "too_old" } }));
		const { mac } = await openCard();

		expect(
			await within(mac).findByText("Update maxd on Mac to see the rules Cursor saved there.")
		).toBeInTheDocument();
	});

	it("shows why it couldn't read them", async () => {
		await renderShell(async () => ({
			status: "error",
			error: { kind: "failed", message: "Cursor's settings aren't JSON" },
		}));
		const { mac } = await openCard();

		expect(await within(mac).findByText("Cursor's settings aren't JSON")).toBeInTheDocument();
		expect(within(mac).getByRole("button", { name: "Copy error" })).toBeInTheDocument();
	});
});
