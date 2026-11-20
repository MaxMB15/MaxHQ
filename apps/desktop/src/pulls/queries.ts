// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The PR reads and actions, shared by every screen that shows them: a
// project's list on each tab and its repo's branches, one PR, its files
// page by page, its conversation and its checks, merging and approving,
// and signing in to GitHub. A read the device refuses fails with the
// reason in words. A
// merge or an approval drops what was read of the PR and the lists, so
// they're read again.

import {
	type QueryClient,
	useInfiniteQuery,
	useMutation,
	useQuery,
	useQueryClient,
} from "@tanstack/react-query";

import { useServices, useView } from "@/app/services";
import type {
	DeviceId,
	MergeMethod,
	ProjectId,
	PullKey,
	PullListState,
} from "@maxhq/shared/bindings";
import { approvePull, mergePull, type PullActed, type PullAt } from "@maxhq/shared/pulls/act";
import { readOf } from "@maxhq/shared/pulls/read";
import { useGithubSignIn as useGithubSignInOn } from "@maxhq/shared/settings/github-sign-in";

export { type GithubSignInState, SIGN_IN_CHECK_MS } from "@maxhq/shared/settings/github-sign-in";

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

/**
 * Signs you in to GitHub on a device you pick. `begin` asks it for a code
 * to enter on GitHub, and the sign-in is done once that device's
 * `github_user` login shows up on the state stream. The phone runs the
 * same, so it lives in the shared package.
 */
export const useGithubSignIn = () => {
	const { source } = useServices();
	const view = useView((s) => s.view);
	return useGithubSignInOn(source, view);
};
