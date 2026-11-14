// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the form that
// copies a project to another device, cloning its repo there and bringing the
// files git ignores, with the env files ticked to start with. The rest is
// MaxHQ's: the copy lands in a new container or a folder clone, the files
// cross over maxd links, and the form follows the copy until it lands, then
// says which files didn't make it.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useEffect, useState } from "react";

import { useServices, useView } from "@/app/services";
import { Bar, CloneBanner } from "@/agent-view/clone-banner";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import { Checkbox } from "@/components/ui/checkbox";
import type { WorkspaceId } from "@maxhq/shared/bindings";
import { copySize, defaultTicks } from "@maxhq/shared/copy/defaults";
import type { CopiedProject, IgnoredFile, TransferProgress } from "@maxhq/shared/data/source";
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
	type CopyPlan,
	copyLanded,
	firstChoices,
	planCopy,
	projectsToCopy,
	sourceChoices,
	targetChoices,
} from "@maxhq/shared/workspaces/copy";
import { canCopy, cloneOf, isLive } from "@maxhq/shared/workspaces/state";

import { closeCopyProject, type CopyRequest, noteMissed, useCopy } from "./copy-store";
import { BACKDROP, CreationSteps, POPUP, Title } from "./new-workspace";

type Planned = Extract<CopyPlan, { ok: true }>;

// The ignored files of the checkout picked, once its device lists them.
type Listing =
	| { readonly state: "loading" }
	| { readonly state: "listed"; readonly files: IgnoredFile[]; readonly truncated: boolean }
	| { readonly state: "failed"; readonly reason: string }
	| { readonly state: "unlisted"; readonly why: string };

// Where a copy is once it starts. `before` holds the workspaces there were,
// so the one it makes can be told apart.
type Run =
	| { readonly phase: "copying"; readonly transfer: TransferProgress | null }
	| { readonly phase: "done"; readonly placed: CopiedProject }
	| { readonly phase: "failed"; readonly reason: string };

type Started = {
	readonly plan: Planned;
	readonly before: ReadonlySet<WorkspaceId>;
	readonly files: number;
	readonly run: Run;
};

const KINDS: { kind: CopyChoices["kind"]; label: string }[] = [
	{
		kind: "container",
		get label() {
			return t("New container");
		},
	},
	{
		kind: "folder",
		get label() {
			return t("Folder");
		},
	},
];

const TRANSFER_STATE: Record<TransferProgress["state"], string> = {
	get uploading() {
		return t("Copying files");
	},
	get waiting() {
		return t("Waiting for room on the device");
	},
	get paused() {
		return t("Paused while the link is down");
	},
	get done() {
		return t("Files copied");
	},
	get failed() {
		return t("The files didn't copy");
	},
};

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
const Copying = ({ started }: { started: Started }) => {
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
						{transfer
							? t("{{value1}}: {{value2}} of {{value3}}", {
									value1: TRANSFER_STATE[transfer.state],
									value2: formatBytes(transfer.sent),
									value3: formatBytes(transfer.total),
								})
							: t("{{countText}} files to copy once it's ready.", {
									count: files,
									countText: formatNumber(files),
								})}
					</span>
					{transfer ? (
						<Bar
							label={t("Files")}
							percent={
								transfer.total > 0
									? Math.round((transfer.sent / transfer.total) * 100)
									: null
							}
						/>
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
const Progress = ({ started, onBack }: { started: Started; onBack: () => void }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const { plan, run } = started;
	const project = view.tables.project.get(plan.project)?.name ?? plan.project;
	const where = deviceName(view, plan.target.device);
	const title =
		run.phase === "done"
			? t("Copied {{project}} to {{where}}", { project: project, where: where })
			: run.phase === "failed"
				? t("The copy didn't finish")
				: t("Copying {{project}} to {{where}}", { project: project, where: where });
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
	const [choices, setChoices] = useState<CopyChoices & { project: string | null }>(() => {
		const from = request.from ? view.tables.workspace.get(request.from) : undefined;
		const project =
			request.project ??
			from?.project ??
			projectsToCopy(view).find((o) => !o.disabled)?.value ??
			null;
		return {
			project,
			...(project
				? firstChoices(view, project, request, live)
				: {
						from: null,
						device: request.device ?? null,
						kind: "folder",
						container: "",
						parent: "",
						folder: "",
					}),
		};
	});
	const [listing, setListing] = useState<{ for: string | null; listing: Listing }>({
		for: null,
		listing: { state: "loading" },
	});
	const [ticks, setTicks] = useState<{ for: string | null; ticked: ReadonlySet<string> }>({
		for: null,
		ticked: new Set(),
	});
	const [started, setStarted] = useState<Started | null>(null);

	const from = choices.from ? view.tables.workspace.get(choices.from) : undefined;
	const fromDevice = from?.device ?? null;
	const lists = fromDevice !== null && canCopy(view, fromDevice);

	// The list follows the checkout picked. A device whose maxd predates
	// copies can't list, so only the repo goes.
	useEffect(() => {
		const id = choices.from;
		if (!id || !lists) return;
		let current = true;
		services.source.listIgnoredFiles(id).then(
			(found) => {
				if (current) setListing({ for: id, listing: { state: "listed", ...found } });
			},
			(error: unknown) => {
				if (!current) return;
				setListing({
					for: id,
					listing: {
						state: "failed",
						reason:
							error instanceof Error
								? error.message
								: t("Couldn't list the ignored files"),
					},
				});
			}
		);
		return () => {
			current = false;
		};
	}, [services, choices.from, lists, t]);

	if (started) {
		return <Progress started={started} onBack={() => setStarted(null)} />;
	}

	const shown: Listing = !choices.from
		? { state: "unlisted", why: "" }
		: !lists
			? {
					state: "unlisted",
					why: t(
						"The maxd on {{value1}} can't list ignored files yet, so only the repo is cloned.",
						{ value1: deviceName(view, fromDevice ?? "") }
					),
				}
			: listing.for === choices.from
				? listing.listing
				: { state: "loading" };
	const files = shown.state === "listed" ? shown.files : [];
	const ticked = ticks.for === choices.from ? ticks.ticked : defaultTicks(files);
	const size = copySize(files, ticked);
	const plan = planCopy(view, choices, live);
	const blocked = !plan.ok
		? plan.reason
		: shown.state === "loading"
			? t("Listing the ignored files.")
			: size.look === "too_large"
				? t("That's more than one copy carries, 2 GB or 20,000 files. Untick some.")
				: null;

	const set = (change: Partial<CopyChoices>) => setChoices((c) => ({ ...c, ...change }));
	const tick = (path: string, on: boolean) => {
		const next = new Set(ticked);
		if (on) next.add(path);
		else next.delete(path);
		setTicks({ for: choices.from, ticked: next });
	};

	const start = async () => {
		if (!plan.ok || blocked || !choices.from) return;
		const paths = files.filter((f) => ticked.has(f.path)).map((f) => f.path);
		const before = new Set(all(view, "workspace").map((w) => w.id));
		const begun: Omit<Started, "run"> = { plan, before, files: paths.length };
		const show = (run: Run) => setStarted({ ...begun, run });
		show({ phase: "copying", transfer: null });
		try {
			// Sent first, so the clone in a new container already commits as
			// this Mac's git identity.
			if (plan.target.type === "container") {
				await sendFirstSettings(services, plan.project, plan.target.device);
			}
			const placed = await services.source.copyProject(
				choices.from,
				plan.target,
				paths,
				(transfer) => show({ phase: "copying", transfer })
			);
			const landed = copyLanded(services.view.getState().view, plan, before);
			if (landed && placed.failed.length > 0) noteMissed(services, landed.id, placed.failed);
			show({ phase: "done", placed });
		} catch (error) {
			console.warn("Couldn't copy the project", error);
			show({
				phase: "failed",
				reason: error instanceof Error ? error.message : t("Couldn't reach the device"),
			});
		}
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
					{KINDS.map((k) => (
						<button
							key={k.kind}
							type="button"
							aria-pressed={choices.kind === k.kind}
							onClick={() => set({ kind: k.kind })}
							className={cn(
								"h-6 flex-1 rounded-[5px] text-xs",
								choices.kind === k.kind
									? "bg-border font-[550] text-foreground"
									: "text-muted-foreground hover:text-secondary-foreground"
							)}
						>
							{k.label}
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
								{t("{{files}} of {{length}}{{value3}}", {
									files: formatNumber(size.files),
									length: formatNumber(files.length),
									value3: size.bytes > 0 ? `, ${formatBytes(size.bytes)}` : "",
								})}
							</span>
							<button
								type="button"
								className="rounded-[5px] px-1.5 text-xs text-secondary-foreground hover:bg-accent"
								onClick={() =>
									setTicks({
										for: choices.from,
										ticked:
											size.files === files.length
												? new Set()
												: new Set(files.map((f) => f.path)),
									})
								}
							>
								{size.files === files.length ? t("None") : t("All")}
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
