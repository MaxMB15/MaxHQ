// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The PR reads and actions, shared by every screen that shows them: a
// project's list on each tab and its repo's branches, one PR, its files
// page by page, its conversation and its checks, merging and approving,
// and signing in to GitHub. A read the device refuses fails with the
// reason in words. A
// merge or an approval drops what was read of the PR and the lists, so
// they're read again.

import { t } from "@maxhq/shared/i18n";

import {
	type QueryClient,
	useInfiniteQuery,
	useMutation,
	useQuery,
	useQueryClient,
} from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { useServices, useView } from "@/app/services";
import type {
	DeviceId,
	MergeMethod,
	ProjectId,
	PullKey,
	PullListState,
	SignInId,
	Timestamp,
} from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { approvePull, mergePull, type PullActed, type PullAt } from "@maxhq/shared/pulls/act";
import { githubLoginOn } from "@maxhq/shared/pulls/devices";
import { readOf } from "@maxhq/shared/pulls/read";
import { errorText } from "@maxhq/shared/timeline/store";

const PULLS = "pulls";

/** What every query of one PR's reads starts with. Repositories compare in lower case. */
const pullKey = (key: PullKey) => [PULLS, "pull", key.repo.toLowerCase(), key.number];

/** Drops what was read of a PR and every list, so they're read again. */
const dropPull = (queries: QueryClient, key: PullKey) =>
	Promise.all([
		queries.invalidateQueries({ queryKey: pullKey(key) }),
		queries.invalidateQueries({ queryKey: [PULLS, "list"] }),
	]);

/** A project's PRs on one tab, newest change first, read again every `everyMs` if given. */
export const usePulls = (
	project: ProjectId | undefined,
	state: PullListState,
	everyMs?: number
) => {
	const { source } = useServices();
	return useQuery({
		queryKey: [PULLS, "list", project, state],
		queryFn: async () => readOf(await source.listPulls(project!, state), "pulls").pulls,
		enabled: project !== undefined,
		refetchInterval: everyMs ?? false,
	});
};

/** A project's repository's branches and its default one, for the new-agent form. */
export const useBranches = (project: ProjectId | undefined) => {
	const { source } = useServices();
	return useQuery({
		queryKey: [PULLS, "branches", project],
		queryFn: async () => readOf(await source.listBranches(project!), "branches"),
		enabled: project !== undefined,
	});
};

/** One PR in full, read on `device`. */
export const usePull = (device: DeviceId | undefined, key: PullKey | undefined) => {
	const { source } = useServices();
	return useQuery({
		queryKey: key ? [...pullKey(key), "detail", device] : [PULLS, "none"],
		queryFn: async () =>
			readOf(await source.getPull(device!, key!.repo, key!.number), "pull").pull,
		enabled: device !== undefined && key !== undefined,
	});
};

/** A PR's changed files, a page at a time. `fetchNextPage` reads the next one. */
export const usePullFiles = (device: DeviceId | undefined, key: PullKey | undefined) => {
	const { source } = useServices();
	return useInfiniteQuery({
		queryKey: key ? [...pullKey(key), "files", device] : [PULLS, "none"],
		queryFn: async ({ pageParam }) =>
			readOf(
				await source.pullFiles(device!, key!.repo, key!.number, pageParam),
				"pull_files"
			),
		initialPageParam: 1,
		getNextPageParam: (last, pages) => (last.more ? pages.length + 1 : undefined),
		enabled: device !== undefined && key !== undefined,
	});
};

/** A PR's conversation, oldest first. */
export const usePullComments = (device: DeviceId | undefined, key: PullKey | undefined) => {
	const { source } = useServices();
	return useQuery({
		queryKey: key ? [...pullKey(key), "comments", device] : [PULLS, "none"],
		queryFn: async () =>
			readOf(await source.pullComments(device!, key!.repo, key!.number), "pull_comments")
				.comments,
		enabled: device !== undefined && key !== undefined,
	});
};

/** A PR's checks on its head, and how they stand together. */
export const usePullChecks = (device: DeviceId | undefined, key: PullKey | undefined) => {
	const { source } = useServices();
	return useQuery({
		queryKey: key ? [...pullKey(key), "checks", device] : [PULLS, "none"],
		queryFn: async () => {
			const { state, checks } = readOf(
				await source.pullChecks(device!, key!.repo, key!.number),
				"pull_checks"
			);
			return { state, checks };
		},
		enabled: device !== undefined && key !== undefined,
	});
};

/**
 * Merges a PR as you at the head the viewer showed, then drops what was
 * read of it. The result says done, queued or why not.
 */
export const useMergePull = () => {
	const { source, view } = useServices();
	const queries = useQueryClient();
	return useMutation({
		mutationFn: ({ pull, method }: { pull: PullAt; method: MergeMethod }): Promise<PullActed> =>
			mergePull(source, view.getState().view, Date.now(), pull, method),
		onSettled: (_acted, _error, { pull }) => dropPull(queries, pull.key),
	});
};

/**
 * Approves a PR's head as you, then drops what was read of it. The result
 * says done or why not.
 */
export const useApprovePull = () => {
	const { source, view } = useServices();
	const queries = useQueryClient();
	return useMutation({
		mutationFn: ({ pull }: { pull: PullAt }): Promise<PullActed> =>
			approvePull(source, view.getState().view, Date.now(), pull),
		onSettled: (_acted, _error, { pull }) => dropPull(queries, pull.key),
	});
};

const GITHUB_PAGES = "https://github.com/";

/** A code waiting to be entered on GitHub. */
type Waiting = {
	readonly device: DeviceId;
	readonly signIn: SignInId;
	readonly userCode: string;
	readonly verificationUri: string;
	readonly expiresAt: Timestamp;
	/** When the device stored the login it held before, if it held one. */
	readonly before: Timestamp | null;
};

/** Where a GitHub sign-in stands, for the dialog. */
export type GithubSignInState =
	| { readonly state: "idle" }
	| { readonly state: "starting"; readonly device: DeviceId }
	/** Waiting for you to enter `userCode` at `verificationUri`. */
	| {
			readonly state: "waiting";
			readonly device: DeviceId;
			readonly userCode: string;
			readonly verificationUri: string;
			readonly expiresAt: Timestamp;
	  }
	/** The device holds your sign-in now. */
	| { readonly state: "done"; readonly device: DeviceId; readonly login: string }
	/** The code ran out with no sign-in. Begin again to retry. */
	| { readonly state: "expired"; readonly device: DeviceId }
	/** The device wouldn't start one, or GitHub turned it down. */
	| { readonly state: "failed"; readonly device: DeviceId; readonly reason: string };

type Started =
	| { readonly state: "idle" }
	| { readonly state: "starting"; readonly device: DeviceId }
	| { readonly state: "waiting"; readonly code: Waiting }
	| { readonly state: "failed"; readonly device: DeviceId; readonly reason: string };

/** How often a waiting sign-in asks its device whether GitHub refused it. */
export const SIGN_IN_CHECK_MS = 2000;

/**
 * Signs you in to GitHub on a device you pick. `begin` asks it for a code
 * to enter on GitHub, and the sign-in is done once that device's
 * `github_user` login shows up on the state stream. While it waits, the
 * device is asked now and then, so a code GitHub turned down says why.
 * A device too old to answer says nothing, and its code shows as expired
 * once its time runs out.
 */
export const useGithubSignIn = () => {
	const { source } = useServices();
	const view = useView((s) => s.view);
	const [started, setStarted] = useState<Started>({ state: "idle" });
	const now = useNow(1000);

	const begin = async (device: DeviceId) => {
		const before = githubLoginOn(view, device)?.storedAt ?? null;
		setStarted({ state: "starting", device });
		const result = await source.beginGithubSignIn(device, null);
		if (result.status === "error") {
			setStarted({ state: "failed", device, reason: errorText(result.error) });
		} else if (result.data.status === "device_code") {
			const code = result.data;
			// The device names this page and the app opens it for you, so it has to be GitHub's.
			if (!code.verification_uri.startsWith(GITHUB_PAGES)) {
				setStarted({
					state: "failed",
					device,
					reason: t("The device named a sign-in page that isn't on GitHub."),
				});
				return;
			}
			setStarted({
				state: "waiting",
				code: {
					device,
					signIn: code.sign_in,
					userCode: code.user_code,
					verificationUri: code.verification_uri,
					expiresAt: code.expires_at,
					before,
				},
			});
		} else {
			const reason =
				result.data.status === "refused"
					? result.data.reason
					: t("The device didn't give a code.");
			setStarted({ state: "failed", device, reason });
		}
	};

	const waitingOn = started.state === "waiting" ? started.code : null;
	useEffect(() => {
		if (!waitingOn) return;
		const { device, signIn } = waitingOn;
		const timer = setInterval(() => {
			void source.checkGithubSignIn(device, signIn).then((result) => {
				if (result.status !== "ok" || result.data.status !== "refused") return;
				const reason = result.data.reason;
				setStarted((now) =>
					now.state === "waiting" && now.code.signIn === signIn
						? { state: "failed", device, reason }
						: now
				);
			});
		}, SIGN_IN_CHECK_MS);
		return () => clearInterval(timer);
	}, [source, waitingOn]);

	/** Stops waiting for the code, and has the device forget it. */
	const cancel = async () => {
		if (started.state === "waiting") {
			await source.cancelGithubSignIn(started.code.device, started.code.signIn);
		}
		setStarted({ state: "idle" });
	};

	const state = ((): GithubSignInState => {
		if (started.state !== "waiting") return started;
		const { device, userCode, verificationUri, expiresAt, before } = started.code;
		const held = githubLoginOn(view, device);
		if (held && held.storedAt !== before) return { state: "done", device, login: held.login };
		if (now >= expiresAt) return { state: "expired", device };
		return { state: "waiting", device, userCode, verificationUri, expiresAt };
	})();

	return { state, begin, cancel };
};
