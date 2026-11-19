// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the form that
// copies a project to another device, cloning its repo there and bringing the
// files git ignores, with the env files ticked to start with. The rest is
// MaxHQ's: the copy lands in a new container or a folder clone, the files
// cross over maxd links, and the form follows the copy until it lands, then
// says which files didn't make it.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useEffect, useState } from "react";

import { useServices, useView } from "@/app/services";
import { Bar, CloneBanner } from "@/agent-view/clone-banner";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import { Checkbox } from "@/components/ui/checkbox";
import { copySize } from "@maxhq/shared/copy/defaults";
import type { CopiedProject, IgnoredFile } from "@maxhq/shared/data/source";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { Picker } from "@/new-agent/picker";
import { FIELD, Label, PRIMARY, SECONDARY } from "@/settings/controls";
import { sendFirstSettings } from "@/settings/project";
import { sentence } from "@maxhq/shared/status/format";
import { all } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";
import {
	checkoutOf,
	containerHint,
	type CopyChoices,
	copyLanded,
	firstChoices,
	planCopy,
	projectsToCopy,
	sourceChoices,
	targetChoices,
} from "@maxhq/shared/workspaces/copy";
import {
	allOrNone,
	allTicked,
	COPY_KINDS,
	copyBlocked,
	type CopyFormChoices,
	type CopyRun,
	copyTitle,
	kindLabel,
	listedFiles,
	listsIgnored,
	type ListingFor,
	loadListing,
	openingChoices,
	runCopy,
	type StartedCopy,
	shownListing,
	tickedFor,
	tickedPaths,
	tickedText,
	type TicksFor,
	transferPercent,
	transferText,
	withTick,
} from "@maxhq/shared/workspaces/copy-form";
import { keepMissed } from "@maxhq/shared/workspaces/missed";
import { cloneOf, isLive } from "@maxhq/shared/workspaces/state";

import { closeCopyProject, type CopyRequest, useCopy } from "./copy-store";
import { BACKDROP, CreationSteps, POPUP, Title } from "./new-workspace";

/** One ignored file you can tick, with its size. */
const FileRow = ({
	file,
	ticked,
	onTick,
}: {
	file: IgnoredFile;
	ticked: boolean;
	onTick: (on: boolean) => void;
}) => {
	useI18n();
	return (
		<label className="flex cursor-pointer items-center gap-2 border-b border-sidebar-border px-2.5 py-1.5 last:border-b-0">
			<Checkbox aria-label={file.path} checked={ticked} onCheckedChange={onTick} />
			<span className="min-w-0 flex-1 truncate font-mono text-xs text-foreground">
				{file.path}
			</span>
			<span className="shrink-0 text-xs text-muted-foreground tabular-nums">
				{formatBytes(file.size)}
			</span>
		</label>
	);
};

/** The copy on its way: the new workspace being made or cloned, then the files. */
const Copying = ({ started }: { started: StartedCopy }) => {
	const { t } = useI18n();

	const { plan, before, files, run } = started;
	const landed = useView((s) => copyLanded(s.view, plan, before)?.id ?? null);
	const cloning = useView((s) => {
		const workspace = landed ? s.view.tables.workspace.get(landed) : undefined;
		return !!workspace && cloneOf(workspace) !== null;
	});
	const where = useView((s) => deviceName(s.view, plan.target.device));
	const transfer = run.phase === "copying" ? run.transfer : null;
	return (
		<>
			{!landed ? (
				<p role="status" className="text-[12.5px] text-muted-foreground">
					{t("Waiting for {{where}} to start it.", { where: where })}
				</p>
			) : plan.target.type === "container" ? (
				<CreationSteps id={landed} planned={plan.where} />
			) : cloning ? (
				<CloneBanner workspace={landed} />
			) : (
				<p className="text-[12.5px] text-status-ready-text">
					{t("Cloned into {{where}}.", { where: plan.where })}
				</p>
			)}
			{files > 0 ? (
				<div className="flex flex-col gap-2">
					<span className="text-[12.5px] text-foreground">
						{transferText(transfer, files)}
					</span>
					{transfer ? (
						<Bar label={t("Files")} percent={transferPercent(transfer)} />
					) : null}
				</div>
			) : null}
		</>
	);
};

/** What a finished copy placed, and the files that didn't make it. */
const Result = ({ placed, files }: { placed: CopiedProject; files: number }) => {
	const { t } = useI18n();

	if (files === 0) {
		return (
			<p role="status" className="text-[12.5px] text-muted-foreground">
				{t("The repo is cloned. No ignored files were ticked.")}
			</p>
		);
	}
	const missed = placed.failed.length;
	return (
		<div role="status" className="flex flex-col gap-2">
			<p className="text-[12.5px] text-foreground">
				{t("{{countText}} files copied.", {
					count: placed.written,
					countText: formatNumber(placed.written),
				})}
			</p>
			{missed > 0 ? (
				<>
					<p className="text-[12.5px] text-status-error-text">
						{t("{{countText}} files weren't copied:", {
							count: missed,
							countText: formatNumber(missed),
						})}
					</p>
					<ul className="max-h-40 overflow-y-auto rounded-[7px] border border-sidebar-border">
						{placed.failed.map((f) => (
							<li
								key={f.path}
								className="flex items-baseline gap-2 border-b border-sidebar-border px-2.5 py-1.5 last:border-b-0"
							>
								<span className="min-w-0 truncate font-mono text-xs text-foreground">
									{f.path}
								</span>
								<span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
									{sentence(f.reason)}
								</span>
							</li>
						))}
					</ul>
				</>
			) : null}
		</div>
	);
};

/** The copy once it started, from the first step to how it ended. */
const Progress = ({ started, onBack }: { started: StartedCopy; onBack: () => void }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const { plan, run } = started;
	const title = copyTitle(view, plan, run);
	return (
		<div className="flex flex-col gap-4">
			<div className="flex flex-col gap-1">
				<Title>{title}</Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{run.phase === "copying" ? (
						t("It keeps going if you close this.")
					) : (
						<span className="font-mono text-xs">{plan.where}</span>
					)}
				</Dialog.Description>
			</div>

			{run.phase === "done" ? (
				<Result placed={run.placed} files={started.files} />
			) : run.phase === "failed" ? (
				<p role="alert" className="text-[12.5px] text-status-error-text">
					{`${sentence(run.reason)}.`}
				</p>
			) : (
				<Copying started={started} />
			)}

			<div className="flex justify-end gap-2">
				{run.phase === "failed" ? (
					<button type="button" className={SECONDARY} onClick={onBack}>
						{t("Back")}
					</button>
				) : null}
				<Dialog.Close className={run.phase === "copying" ? SECONDARY : PRIMARY}>
					{run.phase === "copying" ? t("Close") : t("Done")}
				</Dialog.Close>
			</div>
		</div>
	);
};

const Form = ({ request }: { request: CopyRequest }) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const live = (id: string) => isLive(view, id, now);
	const [choices, setChoices] = useState<CopyFormChoices>(() =>
		openingChoices(view, request, live)
	);
	const [listing, setListing] = useState<ListingFor>({
		for: null,
		listing: { state: "loading" },
	});
	const [ticks, setTicks] = useState<TicksFor>({ for: null, ticked: new Set() });
	const [started, setStarted] = useState<StartedCopy | null>(null);

	const from = choices.from ? view.tables.workspace.get(choices.from) : undefined;
	const lists = listsIgnored(view, choices.from);

	// The list follows the checkout picked. A device whose maxd predates
	// copies can't list, so only the repo goes.
	useEffect(() => {
		const id = choices.from;
		if (!id || !lists) return;
		let current = true;
		void loadListing(services.source, id).then((listing) => {
			if (current) setListing({ for: id, listing });
		});
		return () => {
			current = false;
		};
	}, [services, choices.from, lists]);

	if (started) {
		return <Progress started={started} onBack={() => setStarted(null)} />;
	}

	const shown = shownListing(view, choices.from, listing);
	const files = listedFiles(shown);
	const ticked = tickedFor(choices.from, ticks, files);
	const size = copySize(files, ticked);
	const plan = planCopy(view, choices, live);
	const blocked = copyBlocked(plan, shown, size);

	const set = (change: Partial<CopyChoices>) => setChoices((c) => ({ ...c, ...change }));
	const tick = (path: string, on: boolean) =>
		setTicks({ for: choices.from, ticked: withTick(ticked, path, on) });

	const start = async () => {
		if (!plan.ok || blocked || !choices.from) return;
		const paths = tickedPaths(files, ticked);
		const before = new Set(all(view, "workspace").map((w) => w.id));
		const begun: Omit<StartedCopy, "run"> = { plan, before, files: paths.length };
		const placed = await runCopy(
			services.source,
			choices.from,
			plan,
			paths,
			(run: CopyRun) => setStarted({ ...begun, run }),
			// Sent first, so the clone in a new container already commits as
			// this Mac's git identity.
			plan.target.type === "container"
				? () => sendFirstSettings(services, plan.project, plan.target.device)
				: undefined
		);
		keepMissed(services.source, services.view.getState().view, plan, before, placed);
	};

	return (
		<form
			className="flex flex-col gap-4"
			onSubmit={(e) => {
				e.preventDefault();
				void start();
			}}
		>
			<div className="flex flex-col gap-1">
				<Title>{t("Copy to another device")}</Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{t(
						"Clones the project's repo on the device and brings the files you tick, which git doesn't track."
					)}
				</Dialog.Description>
			</div>

			<div className="flex flex-wrap gap-2">
				<Picker
					label={t("Project")}
					value={choices.project ?? ""}
					options={projectsToCopy(view)}
					onChange={(project) =>
						setChoices((c) => ({
							...c,
							project,
							...firstChoices(view, project, { device: c.device }, live),
						}))
					}
				/>
				<Picker
					label={t("From")}
					value={choices.from ?? ""}
					options={choices.project ? sourceChoices(view, choices.project) : []}
					onChange={(id) => set({ from: id })}
					disabled={!choices.project}
				/>
				<Picker
					label={t("To")}
					value={choices.device ?? ""}
					options={targetChoices(view, live)}
					onChange={(device) => set({ device })}
				/>
			</div>
			{from ? (
				<p className="-mt-2 truncate font-mono text-[11.5px] text-muted-foreground">
					{checkoutOf(from)}
				</p>
			) : null}

			<div className="flex flex-col gap-1.5">
				<Label>{t("Copy into")}</Label>
				<div
					role="group"
					aria-label={t("Copy into")}
					className="flex gap-0.5 rounded-[7px] border border-sidebar-border bg-background p-0.5"
				>
					{COPY_KINDS.map((kind) => (
						<button
							key={kind}
							type="button"
							aria-pressed={choices.kind === kind}
							onClick={() => set({ kind })}
							className={cn(
								"h-6 flex-1 rounded-[5px] text-xs",
								choices.kind === kind
									? "bg-border font-[550] text-foreground"
									: "text-muted-foreground hover:text-secondary-foreground"
							)}
						>
							{kindLabel(kind)}
						</button>
					))}
				</div>
			</div>

			{choices.kind === "container" ? (
				<label className="flex flex-col gap-1.5">
					<Label>{t("Container name")}</Label>
					<span className="flex items-center gap-1.5">
						<span className="font-mono text-xs text-muted-foreground">mhq-</span>
						<input
							aria-label={t("Container name")}
							value={choices.container}
							onChange={(e) => set({ container: e.target.value })}
							placeholder={
								choices.project ? containerHint(view, choices.project) : "name"
							}
							spellCheck={false}
							autoComplete="off"
							className={cn(FIELD, "font-mono text-xs")}
						/>
					</span>
				</label>
			) : (
				<div className="grid grid-cols-[minmax(0,1fr)_minmax(0,0.6fr)] gap-2">
					<label className="flex min-w-0 flex-col gap-1.5">
						<Label>{t("Parent folder")}</Label>
						<input
							value={choices.parent}
							onChange={(e) => set({ parent: e.target.value })}
							placeholder="/home/you/code"
							spellCheck={false}
							autoComplete="off"
							className={cn(FIELD, "font-mono text-xs")}
						/>
					</label>
					<label className="flex min-w-0 flex-col gap-1.5">
						<Label>{t("Folder name")}</Label>
						<input
							value={choices.folder}
							onChange={(e) => set({ folder: e.target.value })}
							spellCheck={false}
							autoComplete="off"
							className={cn(FIELD, "font-mono text-xs")}
						/>
					</label>
				</div>
			)}

			<div className="flex flex-col gap-1.5">
				<div className="flex items-center gap-2">
					<span className="flex-1">
						<Label>{t("Ignored files to copy")}</Label>
					</span>
					{files.length > 0 ? (
						<>
							<span className="text-xs text-muted-foreground tabular-nums">
								{tickedText(size, files.length)}
							</span>
							<button
								type="button"
								className="rounded-[5px] px-1.5 text-xs text-secondary-foreground hover:bg-accent"
								onClick={() =>
									setTicks({ for: choices.from, ticked: allOrNone(files, size) })
								}
							>
								{allTicked(files, size) ? t("None") : t("All")}
							</button>
						</>
					) : null}
				</div>
				{shown.state === "failed" ? (
					<p className="text-xs text-status-error-text">{`${sentence(shown.reason)}.`}</p>
				) : shown.state === "unlisted" ? (
					shown.why ? (
						<p className="text-xs text-muted-foreground">{shown.why}</p>
					) : null
				) : files.length === 0 ? (
					<p className="text-xs text-muted-foreground">
						{shown.state === "listed"
							? t("git ignores no files here, so only the repo is cloned.")
							: t("Listing the files git ignores.")}
					</p>
				) : (
					<div
						role="group"
						aria-label={t("Ignored files")}
						className="max-h-52 overflow-y-auto rounded-[7px] border border-sidebar-border bg-subpanel"
					>
						{files.map((file) => (
							<FileRow
								key={file.path}
								file={file}
								ticked={ticked.has(file.path)}
								onTick={(on) => tick(file.path, on)}
							/>
						))}
					</div>
				)}
				{shown.state === "listed" && shown.truncated ? (
					<p className="text-xs text-muted-foreground">
						{t("Showing the first {{length}} files git ignores.", {
							length: formatNumber(files.length),
						})}
					</p>
				) : null}
				{size.look === "large" ? (
					<p className="text-xs text-status-connecting-text">
						{t("That's {{value1}}, over 100 MB, so the copy takes a while.", {
							value1: formatBytes(size.bytes),
						})}
					</p>
				) : null}
				{files.length > 0 ? (
					<p className="text-xs text-muted-foreground">
						{t(
							"The env files start ticked. Folders git ignores whole, like node_modules, are left out."
						)}
					</p>
				) : null}
			</div>

			{blocked ? (
				<p role="status" className="text-[11.5px] text-muted-foreground">
					{blocked}
				</p>
			) : null}

			<div className="flex items-center justify-end gap-2">
				<Dialog.Close className={SECONDARY}>{t("Cancel")}</Dialog.Close>
				<button type="submit" className={PRIMARY} disabled={blocked !== null}>
					{t("Copy")}
				</button>
			</div>
		</form>
	);
};

/** The copy form, opened from a project's menu or a workspace list. */
export const CopyProjectHost = () => {
	useI18n();

	const services = useServices();
	const open = useCopy((s) => s.open);
	const opened = useCopy((s) => s.opened);
	const request = useCopy((s) => s.request);
	return (
		<Dialog.Root
			open={open}
			onOpenChange={(next) => {
				if (!next) closeCopyProject(services);
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={POPUP}>
					<Form key={opened} request={request} />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
