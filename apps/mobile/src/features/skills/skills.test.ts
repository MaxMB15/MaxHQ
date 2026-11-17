// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { DeviceId, SkillLibrary, SkillProposal } from "@maxhq/shared/bindings";
import { mockSource } from "@maxhq/shared/data/mock-source";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import { skillLibrary, skillProposals } from "@maxhq/shared/skills/library";
import { NOW } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { approveHolder, mayAnswer, shownDevice } from "./skills";

const OTHER = "dev-other" as DeviceId;

const load = async () =>
	applySnapshot(
		await mockSource({ now: () => NOW, tick: false, scenario: "skills_proposal" }).snapshot()
	);

describe("the device a project's Skills screen shows", () => {
	it("keeps your pick, else starts on the library's device, else the first", async () => {
		const library = skillLibrary(await load())!;
		const elsewhere: SkillLibrary = { ...library, device: OTHER };
		const others = [DEVBOX, MAC].filter((d) => d !== library.device);
		expect(shownDevice([...others, library.device], library, null)).toBe(library.device);
		expect(shownDevice([DEVBOX, MAC], library, DEVBOX)).toBe(DEVBOX);
		expect(shownDevice([DEVBOX, MAC], elsewhere, null)).toBe(DEVBOX);
		expect(shownDevice([MAC], library, OTHER)).toBe(MAC);
		expect(shownDevice([], library, DEVBOX)).toBeNull();
		expect(shownDevice([DEVBOX], undefined, null)).toBe(DEVBOX);
	});
});

describe("answering a proposal from the phone", () => {
	it("reaches the library's device too when it isn't the proposal's", async () => {
		const view = await load();
		const library = skillLibrary(view)!;
		const [proposal] = skillProposals(view);
		const here: SkillProposal = { ...proposal!, device: library.device };
		const there: SkillProposal = { ...proposal!, device: OTHER };
		expect(approveHolder(library, here)).toBeNull();
		expect(approveHolder(library, there)).toBe(library.device);
		expect(approveHolder(undefined, there)).toBeNull();
	});

	it("needs the switch on the proposal's device, and on the holder's to approve", () => {
		expect(mayAnswer("reject", MAC, true, false)).toBe(true);
		expect(mayAnswer("reject", null, false, true)).toBe(false);
		expect(mayAnswer("approve", null, true, false)).toBe(true);
		expect(mayAnswer("approve", MAC, true, false)).toBe(false);
		expect(mayAnswer("approve", MAC, true, true)).toBe(true);
		expect(mayAnswer("approve", MAC, false, true)).toBe(false);
	});
});
