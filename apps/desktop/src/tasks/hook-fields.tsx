// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The editor's parts for tasks that take requests: a webhook's URL, shown
// once, and where it's reached, the signature it checks, the GitHub repo and
// events a task watches, the fields its prompt fills in, and the rules both
// share: filters, replay and coalesce keys, a window that restarts, the
// oldest request it takes and its runs a day.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Plus, X } from "lucide-react";
import type { ReactNode } from "react";

import { Group as TickGroup, Tick } from "@/agent-tools/scope-fields";
import { CopyButton } from "@/components/copy-button";
import { Checkbox } from "@/components/ui/checkbox";
import { Picker } from "@/new-agent/picker";
import { FIELD, Label, SECONDARY } from "@/settings/controls";
import { cn } from "@/lib/utils";
import type { DeviceTasks } from "@maxhq/shared/bindings";
import {
	allowField,
	emptyFilter,
	FILTER_OPS,
	type FilterDraft,
	type FilterOp,
	MAX_FILTERS,
	MAX_WAIT_SECS,
	promptFields,
	type TaskDraft,
	toggled,
	withFilter,
} from "@maxhq/shared/tasks/draft";
import {
	addressNote,
	fieldKind,
	fieldsNote,
	GITHUB_EVENTS,
	GITHUB_LATENCY_NOTE,
} from "@maxhq/shared/tasks/format";

/** A change to some of the draft's fields. */
export type Edit = (patch: Partial<TaskDraft>) => void;

/** A labelled text field. */
export const TextField = ({
	label,
	value,
	onChange,
	placeholder,
	mono,
	type = "text",
	className,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	placeholder?: string;
	mono?: boolean;
	type?: "text" | "password" | "time";
	className?: string;
}) => {
	useI18n();
	return (
		<label className={cn("flex min-w-0 flex-col gap-1.5", className)}>
			<Label>{label}</Label>
			<input
				aria-label={label}
				type={type}
				value={value}
				onChange={(e) => onChange(e.target.value)}
				placeholder={placeholder}
				spellCheck={false}
				autoComplete="off"
				className={cn(FIELD, mono && "font-mono text-xs")}
			/>
		</label>
	);
};

/** A small heading over a group of fields. */
export const Part = ({ title, children }: { title: string; children: ReactNode }) => {
	useI18n();
	return (
		<div
			role="group"
			aria-label={title}
			className="flex flex-col gap-3 border-t border-sidebar-border pt-4"
		>
			<h3 className="text-[12.5px] font-[650] text-foreground">{title}</h3>
			{children}
		</div>
	);
};

const Hint = ({ children }: { children: ReactNode }) => {
	useI18n();
	return <p className="text-[11.5px] text-muted-foreground">{children}</p>;
};

/** A webhook URL, shown only now, with Copy. */
export const HookUrl = ({ url }: { url: string }) => {
	const { t } = useI18n();
	return (
		<div className="flex flex-col gap-1.5 rounded-[9px] border border-primary/40 bg-primary/8 p-3">
			<span className="text-[12px] font-[600] text-foreground">{t("Webhook URL")}</span>
			<div className="flex items-center gap-2">
				<code
					className="selectable min-w-0 flex-1 truncate font-mono text-xs text-foreground"
					title={url}
				>
					{url}
				</code>
				<CopyButton text={url} label={t("Copy URL")} className={SECONDARY} />
			</div>
			<Hint>{t("Copy it now. MaxHQ shows it only once, and Rotate makes a new one.")}</Hint>
		</div>
	);
};

/** Who reaches a device's webhooks, and on Funnel what's missing when it doesn't work. */
export const AddressNote = ({
	reach,
	device,
}: {
	reach: Pick<DeviceTasks, "address" | "port" | "funnel">;
	device: string;
}) => {
	const { t } = useI18n();

	const note = addressNote(reach, device);
	if (note !== null)
		return (
			<Hint>
				<span className="selectable">{note}</span>
			</Hint>
		);
	const { funnel } = reach;
	if (funnel.state !== "needs_setup") return null;
	return (
		<div
			role="status"
			className="flex flex-col gap-2 rounded-[9px] border border-status-busy/40 bg-status-busy/8 p-3"
		>
			<span className="text-[12px] font-[600] text-foreground">
				{t("Funnel needs setting up on {{device}}", { device: device })}
			</span>
			<ul className="flex flex-col gap-1.5">
				{funnel.steps.map((step) => (
					<li
						key={step.reason}
						className="flex flex-col gap-1 text-[11.5px] text-secondary-foreground"
					>
						<span className="selectable">{step.reason}</span>
						{step.command ? (
							<span className="flex items-center gap-2">
								<code
									title={step.command}
									className="selectable min-w-0 flex-1 truncate font-mono text-xs text-foreground"
								>
									{step.command}
								</code>
								<CopyButton
									text={step.command}
									label={t("Copy command")}
									className={SECONDARY}
								/>
							</span>
						) : null}
					</li>
				))}
			</ul>
		</div>
	);
};

/** How the webhook checks who sent a request. */
export const SignatureFields = ({
	draft,
	edit,
	hasSecret,
}: {
	draft: TaskDraft;
	edit: Edit;
	/** Whether the device holds a secret for this task already. */
	hasSecret: boolean;
}) => {
	const { t } = useI18n();
	return (
		<>
			<Tick
				label={t("Check a signature")}
				checked={draft.signed}
				onChange={(signed) => edit({ signed })}
			/>
			{draft.signed ? (
				<>
					<div className="grid grid-cols-[1fr_auto_1fr] items-end gap-2">
						<TextField
							label={t("Signature header")}
							value={draft.header}
							onChange={(header) => edit({ header })}
							mono
						/>
						<Picker
							label={t("Encoding")}
							value={draft.encoding}
							options={[
								{ value: "hex", label: "Hex" },
								{ value: "base64", label: "Base64" },
							]}
							onChange={(encoding) =>
								edit({ encoding: encoding as "hex" | "base64" })
							}
						/>
						<TextField
							label={t("Prefix")}
							value={draft.prefix}
							onChange={(prefix) => edit({ prefix })}
							mono
						/>
					</div>
					<TextField
						label={hasSecret ? t("New signing secret") : t("Signing secret")}
						type="password"
						value={draft.secret}
						onChange={(secret) => edit({ secret })}
						placeholder={
							hasSecret ? t("Leave blank to keep the one the device holds") : ""
						}
					/>
					{hasSecret ? null : (
						<Hint>
							{t(
								"Add the signing secret. Until the device holds one, it refuses every request. The secret goes to the device and never comes back."
							)}
						</Hint>
					)}
				</>
			) : null}
		</>
	);
};

/** Whether the webhook keeps requests while its target's device is away. */
export const HoldField = ({ draft, edit }: { draft: TaskDraft; edit: Edit }) => {
	const { t } = useI18n();
	return (
		<Tick
			label={t("Hold requests up to 24 hours while the target device is offline")}
			checked={draft.hold}
			onChange={(hold) => edit({ hold })}
		/>
	);
};

const FilterRow = ({
	index,
	filter,
	onChange,
	onRemove,
}: {
	index: number;
	filter: FilterDraft;
	onChange: (filter: FilterDraft) => void;
	onRemove: () => void;
}) => {
	const { t } = useI18n();
	return (
		<div
			role="group"
			aria-label={t("Filter {{value1}}", { value1: formatNumber(index + 1) })}
			className="flex items-center gap-2"
		>
			<input
				aria-label={t("Path")}
				value={filter.path}
				onChange={(e) => onChange({ ...filter, path: e.target.value })}
				placeholder="body.action"
				spellCheck={false}
				autoComplete="off"
				className={cn(FIELD, "min-w-0 flex-1 font-mono text-xs")}
			/>
			<Picker
				label={t("Test")}
				value={filter.op}
				options={FILTER_OPS.map((o) => ({ value: o.op, label: o.label }))}
				onChange={(op) => onChange({ ...filter, op: op as FilterOp })}
			/>
			{filter.op === "exists" ? (
				<span className="min-w-0 flex-1" />
			) : (
				<input
					aria-label={t("Value")}
					value={filter.value}
					onChange={(e) => onChange({ ...filter, value: e.target.value })}
					placeholder={filter.op === "in" ? "opened, closed" : "opened"}
					spellCheck={false}
					autoComplete="off"
					className={cn(FIELD, "min-w-0 flex-1 font-mono text-xs")}
				/>
			)}
			<button
				type="button"
				aria-label={t("Remove filter {{value1}}", { value1: formatNumber(index + 1) })}
				onClick={onRemove}
				className="grid size-[30px] shrink-0 place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
			>
				<X aria-hidden size={14} />
			</button>
		</div>
	);
};

/** The placeholders the prompt uses, which are free text, and the free text it may hold. */
export const PromptFields = ({ draft, edit }: { draft: TaskDraft; edit: Edit }) => {
	const { t } = useI18n();

	const github = draft.kind === "github_events";
	const used = promptFields(draft.prompt, github);
	// A webhook from before the list fills in every field, framed.
	const everything = draft.fields === null && !github;
	const allowed = draft.fields ?? [];
	const allow = (expression: string, on: boolean) =>
		edit({ fields: allowField(draft.fields, expression, on) });
	return (
		<>
			{everything ? (
				<div
					role="status"
					className="flex flex-col gap-2 rounded-[9px] border border-status-busy/40 bg-status-busy/8 p-3"
				>
					<span className="text-[12px] font-[600] text-foreground">
						{t("This prompt fills in every field of a request")}
					</span>
					<span className="text-[11.5px] text-secondary-foreground">
						{t(
							"Free text, such as a title or a comment, comes framed as untrusted data, but it can still try to steer the agent. The safe default leaves it out until you allow each field."
						)}
					</span>
					<button
						type="button"
						className={cn(SECONDARY, "self-start")}
						onClick={() => edit({ fields: [] })}
					>
						{t("Use the safe default")}
					</button>
				</div>
			) : null}
			{used.length === 0 ? (
				<Hint>{t("The prompt uses no placeholders.")}</Hint>
			) : (
				<ul aria-label={t("Fields")} className="flex flex-col gap-1.5">
					{used.map((field) => {
						const { expression, free } = field;
						return (
							<li key={expression} className="flex min-w-0 items-center gap-2">
								{free && !everything ? (
									<Checkbox
										aria-label={t("Allow {{expression}}", {
											expression: expression,
										})}
										checked={allowed.includes(expression)}
										onCheckedChange={(on) => allow(expression, on)}
									/>
								) : null}
								<code
									title={expression}
									className="selectable min-w-0 truncate font-mono text-xs text-foreground"
								>
									{expression}
								</code>
								<span
									className={cn(
										"shrink-0 text-[11.5px]",
										free ? "text-status-busy-text" : "text-muted-foreground"
									)}
								>
									{fieldKind(field, github)}
								</span>
							</li>
						);
					})}
				</ul>
			)}
			<Hint>{fieldsNote(github)}</Hint>
		</>
	);
};

/** The rules a webhook and GitHub events share. */
export const RulesFields = ({ draft, edit }: { draft: TaskDraft; edit: Edit }) => {
	const { t } = useI18n();

	const setFilter = (index: number, filter: FilterDraft | null) =>
		edit({ filters: withFilter(draft.filters, index, filter) });
	return (
		<>
			<div className="flex flex-col gap-2">
				<Label>{t("Filters")}</Label>
				{draft.filters.length === 0 ? (
					<Hint>
						{t("Every request runs it. A filter skips the ones that don't match.")}
					</Hint>
				) : (
					draft.filters.map((filter, index) => (
						<FilterRow
							key={filter.id}
							index={index}
							filter={filter}
							onChange={(next) => setFilter(index, next)}
							onRemove={() => setFilter(index, null)}
						/>
					))
				)}
				<button
					type="button"
					className={cn(SECONDARY, "self-start")}
					disabled={draft.filters.length >= MAX_FILTERS}
					onClick={() => edit({ filters: [...draft.filters, emptyFilter()] })}
				>
					<Plus aria-hidden size={13} />
					{t("Add filter")}
				</button>
			</div>
			<div className="grid grid-cols-2 gap-3">
				<TextField
					label={t("Replay key")}
					value={draft.replayKey}
					onChange={(replayKey) => edit({ replayKey })}
					placeholder="{{headers.x-github-delivery}}"
					mono
				/>
				<TextField
					label={t("Coalesce key")}
					value={draft.coalesceKey}
					onChange={(coalesceKey) => edit({ coalesceKey })}
					placeholder="{{body.pull_request.number}}"
					mono
				/>
				<TextField
					label={t("Coalesce window, seconds")}
					value={draft.coalesceSecs}
					onChange={(coalesceSecs) => edit({ coalesceSecs })}
					placeholder="120"
				/>
				<TextField
					label={t("Skip requests older than, minutes")}
					value={draft.maxAge}
					onChange={(maxAge) => edit({ maxAge })}
					placeholder={t("No limit")}
				/>
				<TextField
					label={t("Runs a day")}
					value={draft.dailyRuns}
					onChange={(dailyRuns) => edit({ dailyRuns })}
					placeholder={t("No cap")}
				/>
			</div>
			<div className="flex flex-wrap items-end gap-3">
				<Tick
					label={t("Restart the window with each request")}
					checked={draft.coalesceRestart}
					onChange={(coalesceRestart) => edit({ coalesceRestart })}
				/>
				{draft.coalesceRestart ? (
					<TextField
						label={t("Longest wait, seconds")}
						value={draft.coalesceMaxSecs}
						onChange={(coalesceMaxSecs) => edit({ coalesceMaxSecs })}
						placeholder={String(MAX_WAIT_SECS.default)}
						className="w-[180px]"
					/>
				) : null}
			</div>
			<Hint>
				{t(
					"Replay keys suppress repeat deliveries for 7 days. Requests with the same coalesce key run once, with each of them in the prompt, when the window closes. It counts from the first request, or with restart on from the last, up to the longest wait. A key with no placeholder, such as all, puts every request in one window."
				)}
			</Hint>
		</>
	);
};

/** The repo and events a GitHub events task watches. */
export const GithubFields = ({
	draft,
	edit,
	repos,
}: {
	draft: TaskDraft;
	edit: Edit;
	/** The repos the GitHub App reaches. */
	repos: readonly string[];
}) => {
	const { t } = useI18n();
	return (
		<>
			<Picker
				label={t("Repository")}
				value={draft.repo}
				options={repos.map((repo) => ({ value: repo, label: repo }))}
				onChange={(repo) => edit({ repo })}
				disabled={repos.length === 0}
			/>
			{repos.length === 0 ? (
				<Hint>
					{t(
						"Add repos to the GitHub App in Settings › Accounts › GitHub to watch them."
					)}
				</Hint>
			) : null}
			<TickGroup title={t("Events")}>
				{GITHUB_EVENTS.map(({ event, label }) => (
					<Tick
						key={event}
						label={label}
						checked={draft.events.has(event)}
						onChange={(on) => edit({ events: toggled(draft.events, event, on) })}
					/>
				))}
			</TickGroup>
			<Hint>{t(GITHUB_LATENCY_NOTE)}</Hint>
			<Tick
				label={t("Only for the repo's owners, members and collaborators")}
				checked={draft.repoPeople}
				onChange={(repoPeople) => edit({ repoPeople })}
			/>
			<Hint>
				{t(
					"Issues, pull requests, comments and reviews from anyone else won't run this task. Turn this off and, on a public repo, any stranger can start a run. Events with no author, such as pushes, forks and stars, run either way."
				)}
			</Hint>
		</>
	);
};
