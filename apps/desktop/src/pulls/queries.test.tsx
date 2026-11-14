// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { PullKey } from "@maxhq/shared/bindings";
import {
	MOCK_GITHUB_LOGIN,
	MOCK_PULLS_PROJECT,
	MOCK_REPO,
	SIGN_IN_MS,
} from "@maxhq/shared/data/mock-pulls";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import {
	useApprovePull,
	useGithubSignIn,
	useMergePull,
	usePull,
	usePullChecks,
	usePullComments,
	usePullFiles,
	usePulls,
} from "./queries";

const READY: PullKey = { repo: MOCK_REPO, number: 141 };

/** Renders a hook over the sample data, at a time a test can move. */
const setup = (source: DataSource = mockSource({ now: () => NOW, tick: false })) => {
	const clock = { now: NOW };
	const wrapper = ({ children }: { children: ReactNode }) => (
		<FixedNow value={clock.now}>
			<ServicesProvider source={source} ui={createUiStore()}>
				{children}
			</ServicesProvider>
		</FixedNow>
	);
	return { source, clock, wrapper };
};

afterEach(() => {
	vi.useRealTimers();
});

describe("PR reads", () => {
	it("lists a project's PRs on each tab", async () => {
		const { wrapper } = setup();
		const { result } = renderHook(() => usePulls(MOCK_PULLS_PROJECT, "closed"), { wrapper });
		await waitFor(() =>
			expect(result.current.data?.map((row) => row.pull.state)).toEqual(["merged", "closed"])
		);
	});

	it("waits for a project", () => {
		const { wrapper } = setup();
		const { result } = renderHook(() => usePulls(undefined, "open"), { wrapper });
		expect(result.current.fetchStatus).toBe("idle");
	});

	it("reads one PR, its conversation and its checks", async () => {
		const { wrapper } = setup();
		const { result } = renderHook(
			() => ({
				pull: usePull(DEVBOX, READY),
				comments: usePullComments(DEVBOX, READY),
				checks: usePullChecks(DEVBOX, READY),
			}),
			{ wrapper }
		);
		await waitFor(() => expect(result.current.pull.data?.summary.key).toEqual(READY));
		await waitFor(() => expect(result.current.comments.data?.length).toBeGreaterThan(0));
		await waitFor(() => expect(result.current.checks.data?.state).toBe("passing"));
	});

	it("reads files a page at a time", async () => {
		const { wrapper } = setup();
		const key = { repo: MOCK_REPO, number: 145 };
		const { result } = renderHook(() => usePullFiles(DEVBOX, key), { wrapper });
		await waitFor(() => expect(result.current.data?.pages).toHaveLength(1));
		expect(result.current.hasNextPage).toBe(true);
		await act(() => result.current.fetchNextPage());
		await waitFor(() => expect(result.current.data?.pages).toHaveLength(2));
		expect(result.current.hasNextPage).toBe(false);
	});

	it("fails with the device's reason", async () => {
		const { wrapper } = setup();
		const missing = { repo: "owner/other", number: 1 };
		const { result } = renderHook(() => usePull(DEVBOX, missing), { wrapper });
		await waitFor(() => expect(result.current.error?.message).toBe("GitHub has no such PR."));
	});
});

describe("useGithubSignIn", () => {
	it("keeps the code, then is done once the device holds your login", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { wrapper } = setup();
		const { result } = renderHook(() => useGithubSignIn(), { wrapper });
		await act(() => result.current.begin(MAC));
		expect(result.current.state).toMatchObject({
			state: "waiting",
			device: MAC,
			userCode: "WDJB-MJHT",
			verificationUri: "https://github.com/login/device",
		});
		await act(() => vi.advanceTimersByTimeAsync(SIGN_IN_MS));
		await waitFor(() =>
			expect(result.current.state).toEqual({
				state: "done",
				device: MAC,
				login: MOCK_GITHUB_LOGIN,
			})
		);
	});

	it("won't send you to a sign-in page off GitHub", async () => {
		for (const verification_uri of [
			"https://github.com.example.net/login/device",
			"https://github.com@example.net/login/device",
			"http://github.com/login/device",
			"vscode://file/etc/passwd",
			"javascript:alert(1)",
		]) {
			const source: DataSource = {
				...mockSource({ now: () => NOW, tick: false }),
				beginGithubSignIn: async () => ({
					status: "ok",
					data: {
						status: "device_code",
						sign_in: "sign-in",
						user_code: "WDJB-MJHT",
						verification_uri,
						expires_at: NOW + 60_000,
					},
				}),
			};
			const { wrapper } = setup(source);
			const { result } = renderHook(() => useGithubSignIn(), { wrapper });
			await act(() => result.current.begin(MAC));
			expect(result.current.state).toMatchObject({ state: "failed", device: MAC });
		}
	});

	it("says the code expired once its time passes", async () => {
		const { wrapper, clock } = setup();
		const { result, rerender } = renderHook(() => useGithubSignIn(), { wrapper });
		await act(() => result.current.begin(MAC));
		const waiting = result.current.state;
		if (waiting.state !== "waiting") throw new Error("no code");
		clock.now = waiting.expiresAt;
		rerender();
		expect(result.current.state).toEqual({ state: "expired", device: MAC });
	});

	it("cancels the code on the device", async () => {
		const { wrapper, source } = setup();
		const cancel = vi.spyOn(source, "cancelGithubSignIn");
		const { result } = renderHook(() => useGithubSignIn(), { wrapper });
		await act(() => result.current.begin(MAC));
		await act(() => result.current.cancel());
		expect(cancel).toHaveBeenCalledExactlyOnceWith(MAC, "sign-in-1");
		expect(result.current.state).toEqual({ state: "idle" });
	});

	it("says why the device wouldn't start one", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		vi.spyOn(source, "beginGithubSignIn").mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "No GitHub App to sign in with." },
		});
		const { wrapper } = setup(source);
		const { result } = renderHook(() => useGithubSignIn(), { wrapper });
		await act(() => result.current.begin(MAC));
		expect(result.current.state).toEqual({
			state: "failed",
			device: MAC,
			reason: "No GitHub App to sign in with.",
		});
	});
});

describe("merge and approve", () => {
	it("merges, then reads the list again", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		const { wrapper, source } = setup();
		const due = vi.spyOn(source, "pullsDue");
		const { result } = renderHook(
			() => ({
				open: usePulls(MOCK_PULLS_PROJECT, "open"),
				pull: usePull(DEVBOX, READY),
				merge: useMergePull(),
				signIn: useGithubSignIn(),
			}),
			{ wrapper }
		);
		await act(() => result.current.signIn.begin(MAC));
		await act(() => vi.advanceTimersByTimeAsync(SIGN_IN_MS));
		await waitFor(() => expect(result.current.signIn.state.state).toBe("done"));
		await waitFor(() => expect(result.current.pull.data).toBeDefined());
		const headSha = result.current.pull.data!.summary.pull.head_sha;

		const acted = await act(() =>
			result.current.merge.mutateAsync({ pull: { key: READY, headSha }, method: "squash" })
		);
		expect(acted).toEqual({ state: "done" });
		expect(due).toHaveBeenCalledWith(MAC, MOCK_REPO);
		await waitFor(() =>
			expect(result.current.open.data?.some((row) => row.key.number === 141)).toBe(false)
		);
		await waitFor(() => expect(result.current.pull.data?.summary.pull.state).toBe("merged"));
	});

	it("says you need to sign in first", async () => {
		const { wrapper } = setup();
		const { result } = renderHook(() => useApprovePull(), { wrapper });
		const acted = await act(() =>
			result.current.mutateAsync({ pull: { key: READY, headSha: "x" } })
		);
		expect(acted).toEqual({
			state: "refused",
			reason: "Sign in to GitHub in Settings to approve here.",
		});
	});
});
