// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The copy form's state, which the desktop's dialog and the phone's screen
// share: what it opens with, the ignored files it shows for the checkout
// picked and which of them are ticked, why Copy is off, and how a copy reads
// once it starts, from the files on their way to how it ended.

import { formatNumber, t } from "../i18n";

import { formatBytes } from "../agent-view/format";
import type { DeviceId, ProjectId, WorkspaceId } from "../bindings";
import { type CopySize, defaultTicks } from "../copy/defaults";
import type { CopiedProject, DataSource, IgnoredFile, TransferProgress } from "../data/source";
import type { ViewState } from "../view/replica";
import { deviceName } from "../view/select";

import { type CopyChoices, type CopyPlan, firstChoices, projectsToCopy } from "./copy";
import { canCopy } from "./state";

/** What the form opened for: a project, a checkout, a device to copy onto, or a mix. */
export type CopyRequest = {
	readonly project?: ProjectId | null;
	readonly from?: WorkspaceId | null;
	readonly device?: DeviceId | null;
};

/** What you pick, with the project it copies. */
export type CopyFormChoices = CopyChoices & { readonly project: ProjectId | null };

/** A plan that can go. */
export type PlannedCopy = Extract<CopyPlan, { ok: true }>;

/** The ignored files of the checkout picked, once its device lists them. */
export type Listing =
	| { readonly state: "loading" }
	| { readonly state: "listed"; readonly files: IgnoredFile[]; readonly truncated: boolean }
	| { readonly state: "failed"; readonly reason: string }
	| { readonly state: "unlisted"; readonly why: string };

/** A listing with the checkout it's for, so another pick shows loading until it lists. */
export type ListingFor = { readonly for: WorkspaceId | null; readonly listing: Listing };

/** The ticks with the checkout they're for. Another checkout starts from the default ticks. */
export type TicksFor = { readonly for: WorkspaceId | null; readonly ticked: ReadonlySet<string> };

/** Where a copy is once it starts. */
export type CopyRun =
	| { readonly phase: "copying"; readonly transfer: TransferProgress | null }
	| { readonly phase: "done"; readonly placed: CopiedProject }
	| { readonly phase: "failed"; readonly reason: string };

/**
 * A copy once it started. `before` holds the workspaces there were, so the
 * one it makes can be told apart, and `files` is how many it brings.
 */
export type StartedCopy = {
	readonly plan: PlannedCopy;
	readonly before: ReadonlySet<WorkspaceId>;
	readonly files: number;
	readonly run: CopyRun;
};

/** The two places a copy can land, in the order the form offers them. */
export const COPY_KINDS: readonly CopyChoices["kind"][] = ["container", "folder"];

/** Where a copy lands, in words. */
export const kindLabel = (kind: CopyChoices["kind"]): string =>
	kind === "container" ? t("New container") : t("Folder");

/**
 * What the form opens with: the project asked for, or the checkout's, or
 * the first one that can copy, with its first checkout and target.
 */
export const openingChoices = (
	view: ViewState,
	request: CopyRequest,
	live: (device: DeviceId) => boolean
): CopyFormChoices => {
	const from = request.from ? view.tables.workspace.get(request.from) : undefined;
	const project =
		request.project ??
		from?.project ??
		projectsToCopy(view).find((o) => !o.disabled)?.value ??
		null;
	if (!project) {
		return {
			project: null,
			from: null,
			device: request.device ?? null,
			kind: "folder",
			container: "",
			parent: "",
			folder: "",
		};
	}
	return { project, ...firstChoices(view, project, request, live) };
};

/** Whether the checkout's device lists the files git ignores. A maxd from before copies can't. */
export const listsIgnored = (view: ViewState, from: WorkspaceId | null): boolean => {
	const device = from ? view.tables.workspace.get(from)?.device : undefined;
	return device !== undefined && canCopy(view, device);
};

/** Why a call failed, in words, or `fallback` when the error says nothing. */
export const failureReason = (error: unknown, fallback: string): string =>
	error instanceof Error && error.message ? error.message : fallback;

/** Lists the checkout's ignored files. It never throws: a failure is a listing of its own. */
export const loadListing = (
	source: Pick<DataSource, "listIgnoredFiles">,
	from: WorkspaceId
): Promise<Listing> =>
	source.listIgnoredFiles(from).then(
		(found): Listing => ({ state: "listed", files: found.files, truncated: found.truncated }),
		(error: unknown): Listing => ({
			state: "failed",
			reason: failureReason(error, t("Couldn't list the ignored files")),
		})
	);

/** The listing the form shows for the checkout picked. */
export const shownListing = (
	view: ViewState,
	from: WorkspaceId | null,
	listed: ListingFor
): Listing => {
	if (!from) return { state: "unlisted", why: "" };
	if (!listsIgnored(view, from)) {
		return {
			state: "unlisted",
			why: t(
				"The maxd on {{value1}} can't list ignored files yet, so only the repo is cloned.",
				{ value1: deviceName(view, view.tables.workspace.get(from)?.device ?? "") }
			),
		};
	}
	return listed.for === from ? listed.listing : { state: "loading" };
};

/** The files a listing shows. */
export const listedFiles = (listing: Listing): IgnoredFile[] =>
	listing.state === "listed" ? listing.files : [];

/** The files ticked for the checkout picked: the env files to start with. */
export const tickedFor = (
	from: WorkspaceId | null,
	ticks: TicksFor,
	files: readonly IgnoredFile[]
): ReadonlySet<string> => (ticks.for === from ? ticks.ticked : defaultTicks(files));

/** The ticks with one file ticked or not. */
export const withTick = (ticked: ReadonlySet<string>, path: string, on: boolean): Set<string> => {
	const next = new Set(ticked);
	if (on) next.add(path);
	else next.delete(path);
	return next;
};

/** Whether every file is ticked, so the toggle offers None rather than All. */
export const allTicked = (files: readonly IgnoredFile[], size: CopySize): boolean =>
	size.files === files.length;

/** Every file ticked, or none once every file is. */
export const allOrNone = (files: readonly IgnoredFile[], size: CopySize): Set<string> =>
	allTicked(files, size) ? new Set() : new Set(files.map((f) => f.path));

/** The paths ticked, in the listing's order. */
export const tickedPaths = (files: readonly IgnoredFile[], ticked: ReadonlySet<string>): string[] =>
	files.filter((f) => ticked.has(f.path)).map((f) => f.path);

/** How many files are ticked of how many, and their size once any are. */
export const tickedText = (size: CopySize, total: number): string =>
	t("{{files}} of {{length}}{{value3}}", {
		files: formatNumber(size.files),
		length: formatNumber(total),
		value3: size.bytes > 0 ? `, ${formatBytes(size.bytes)}` : "",
	});

/** Why Copy is off, or null once it can go. */
export const copyBlocked = (plan: CopyPlan, shown: Listing, size: CopySize): string | null =>
	!plan.ok
		? plan.reason
		: shown.state === "loading"
			? t("Listing the ignored files.")
			: size.look === "too_large"
				? t("That's more than one copy carries, 2 GB or 20,000 files. Untick some.")
				: null;

/** A transfer's state in words. */
export const transferStateText = (state: TransferProgress["state"]): string => {
	switch (state) {
		case "uploading":
			return t("Copying files");
		case "waiting":
			return t("Waiting for room on the device");
		case "paused":
			return t("Paused while the link is down");
		case "done":
			return t("Files copied");
		case "failed":
			return t("The files didn't copy");
	}
};

/** How far the files have got, or how many wait while the new workspace gets ready. */
export const transferText = (transfer: TransferProgress | null, files: number): string =>
	transfer
		? t("{{value1}}: {{value2}} of {{value3}}", {
				value1: transferStateText(transfer.state),
				value2: formatBytes(transfer.sent),
				value3: formatBytes(transfer.total),
			})
		: t("{{countText}} files to copy once it's ready.", {
				count: files,
				countText: formatNumber(files),
			});

/** A transfer's share sent, 0 to 100, or null while its size is unknown. */
export const transferPercent = (transfer: TransferProgress): number | null =>
	transfer.total > 0 ? Math.round((transfer.sent / transfer.total) * 100) : null;

/** The copy's title once it starts, by how far it got. */
export const copyTitle = (view: ViewState, plan: PlannedCopy, run: CopyRun): string => {
	const project = view.tables.project.get(plan.project)?.name ?? plan.project;
	const where = deviceName(view, plan.target.device);
	return run.phase === "done"
		? t("Copied {{project}} to {{where}}", { project, where })
		: run.phase === "failed"
			? t("The copy didn't finish")
			: t("Copying {{project}} to {{where}}", { project, where });
};

/**
 * Runs a copy, telling `show` each step: copying, with how far the files
 * got, then done or failed. `first` runs before the copy, such as the
 * desktop sending its git identity. Gives what it placed, or null when it
 * failed.
 */
export const runCopy = async (
	source: Pick<DataSource, "copyProject">,
	from: WorkspaceId,
	plan: PlannedCopy,
	paths: string[],
	show: (run: CopyRun) => void,
	first?: () => Promise<void>
): Promise<CopiedProject | null> => {
	show({ phase: "copying", transfer: null });
	try {
		await first?.();
		const placed = await source.copyProject(from, plan.target, paths, (transfer) =>
			show({ phase: "copying", transfer })
		);
		show({ phase: "done", placed });
		return placed;
	} catch (error) {
		console.warn("Couldn't copy the project", error);
		show({ phase: "failed", reason: failureReason(error, t("Couldn't reach the device")) });
		return null;
	}
};
