// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { DEFAULT_TYPING, writeTypingPrefs } from "@maxhq/shared/editor/typing";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { ComposerNativeProvider } from "./native";
import { fakeNative } from "./testing";
import { setTypingPrefs, TYPING_KEY, useFieldTyping } from "./typing-prefs";

afterEach(() => localStorage.clear());

/** A plain field's typing props, on a Mac with its own spell check or in a browser. */
const renderField = ({ spelling }: { spelling: boolean }) => {
	const { native } = fakeNative({ spelling });
	const wrapper = ({ children }: { children: ReactNode }) => (
		<ComposerNativeProvider value={native}>
			<ServicesProvider
				source={mockSource({ now: () => NOW, tick: false, scenario: "connected" })}
				ui={createUiStore()}
			>
				{children}
			</ServicesProvider>
		</ComposerNativeProvider>
	);
	let services: Services | null = null;
	const hook = renderHook(
		() => {
			services = useServices();
			return useFieldTyping();
		},
		{ wrapper }
	);
	return {
		hook,
		change: (prefs: Parameters<typeof setTypingPrefs>[1]) =>
			act(() => setTypingPrefs(services!, prefs)),
	};
};

describe("a plain field's typing", () => {
	it("has no spell check, autocorrect or capitals in the app by default", () => {
		const { hook } = renderField({ spelling: true });
		expect(hook.result.current).toEqual({
			spellCheck: false,
			autoCorrect: "off",
			autoCapitalize: "off",
		});
	});

	it("keeps the browser's spell check outside the app, still without autocorrect", () => {
		const { hook } = renderField({ spelling: false });
		expect(hook.result.current).toMatchObject({ spellCheck: true, autoCorrect: "off" });
	});

	it("checks and corrects once Correct spelling automatically is on", () => {
		const { hook, change } = renderField({ spelling: true });
		change({ correctSpelling: true });
		expect(hook.result.current).toMatchObject({ spellCheck: true, autoCorrect: "on" });
		change({ checkSpelling: false });
		expect(hook.result.current).toMatchObject({ spellCheck: false, autoCorrect: "off" });
	});

	it("gives every field without its own spell check the same typing, through the page", () => {
		const root = document.documentElement;
		const { hook, change } = renderField({ spelling: true });
		expect(root).toHaveAttribute("spellcheck", "false");
		expect(root).toHaveAttribute("autocorrect", "off");
		change({ correctSpelling: true });
		expect(root).toHaveAttribute("spellcheck", "true");
		expect(root).toHaveAttribute("autocorrect", "on");
		hook.unmount();
		expect(root).not.toHaveAttribute("spellcheck");
	});

	it("starts from the saved settings", () => {
		localStorage.setItem(
			TYPING_KEY,
			writeTypingPrefs({ ...DEFAULT_TYPING, correctSpelling: true })
		);
		const { hook } = renderField({ spelling: true });
		expect(hook.result.current.autoCorrect).toBe("on");
	});
});
