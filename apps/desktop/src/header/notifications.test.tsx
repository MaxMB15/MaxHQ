// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { setBadgePrefs } from "@/native/dock-badge";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import { DEFAULT_BADGE_PREFS } from "@maxhq/shared/view/badge";

import { NotificationBell } from "./notifications";

beforeEach(() => {
	localStorage.clear();
	setBadgePrefs(DEFAULT_BADGE_PREFS);
});

/** The bell over the sample agents, once it counts the ones that need you. */
const renderBell = async () => {
	await act(async () => {
		render(
			<FixedNow value={NOW}>
				<ServicesProvider
					source={mockSource({ now: () => NOW, tick: false })}
					ui={createUiStore()}
				>
					<NotificationBell />
				</ServicesProvider>
			</FixedNow>
		);
	});
	return screen.findByRole("button", { name: /^\d+ agents? needs? you$/ });
};

describe("the bell", () => {
	it("shows its count only while Show the badge is on, and opens either way", async () => {
		const bell = await renderBell();
		const count = bell.textContent!;
		expect(count).toMatch(/^\d+$/);

		act(() => setBadgePrefs({ show: false }));
		expect(bell).toHaveTextContent(/^$/);
		expect(bell).toHaveAccessibleName("Notifications");

		fireEvent.click(bell);
		const now = await screen.findByRole("region", { name: "Now" });
		expect(within(now).getAllByRole("listitem")).toHaveLength(Number(count));

		act(() => setBadgePrefs({ show: true }));
		expect(bell).toHaveTextContent(count);
		expect(bell).toHaveAccessibleName(/^\d+ agents? needs? you$/);
	});
});
