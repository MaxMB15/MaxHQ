// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The ACP agents in a device's settings: the agent programs you set up to
// run there, each with what its last start found, and the form that adds
// or changes one. A variable's value is typed into a password field that
// only Save reads. Save sends it and clears the field, and the device never
// sends it back, so a value it holds shows only as set, with Replace and
// Remove.

import { type FormEvent, useRef, useState } from "react";

import { useServices } from "@/app/services";
import { CopyIcon } from "@/components/copy-field";
import { Switch } from "@/components/ui/switch";
import type { AcpAgent, DeviceId } from "@maxhq/shared/bindings";
import {
	type AcpDraft,
	accountOfAgent,
	acpAgentsOn,
	draftOf,
	newArgument,
	newVariable,
	planAcpSave,
	probeLine,
	runsAcpAgents,
	sendsValue,
	type VariableRow,
} from "@maxhq/shared/settings/acp-agents";
import { sentence } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";
import { cn } from "@/lib/utils";

import {
	ConfirmButton,
	ErrorCopy,
	FIELD,
	Label,
	PRIMARY,
	Row,
	SECONDARY,
	Section,
	StatusNote,
} from "./controls";
import { failureOf, type Note, scrub } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const scrubAll = (text: string, values: readonly string[]): string =>
	values.reduce((shown, value) => scrub(shown, value), text);

/** The ACP agents section, or nothing when the device's `maxd` doesn't run them. */
export const AcpAgents = ({
	device,
	name,
	live,
	view,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
	view: ViewState;
}) => {
	useI18n();
	const { source } = useServices();
	const [editing, setEditing] = useState<AcpDraft | null>(null);
	const [note, setNote] = useState<{ id: string; note: Note } | null>(null);
	if (!runsAcpAgents(view, device)) return null;
	const agents = acpAgentsOn(view, device);
	// Running agents in the device's containers needs Incus there.
	const containers = view.tables.device.get(device)?.workspace_host != null;

	const remove = async (agent: AcpAgent) => {
		setNote({ id: agent.id, note: { text: translate("Removing"), error: false } });
		try {
			const failure = failureOf(await source.deleteAcpAgent(device, agent.id));
			setNote(
				failure ? { id: agent.id, note: { text: sentence(failure), error: true } } : null
			);
		} catch (error) {
			console.warn(translate("Couldn't remove the ACP agent"), error);
			setNote({
				id: agent.id,
				note: { text: translate("Couldn't remove it. Try again."), error: true },
			});
		}
	};

	return (
		<Section
			title={translate("ACP agents")}
			label={translate("ACP agents on {{name}}", { name: name })}
			hint={translate("Agent programs that speak the Agent Client Protocol")}
		>
			{agents.length === 0 && !editing ? (
				<Row
					title={translate("No ACP agents yet")}
					sub={
						live
							? translate(
									"Add an agent program that runs on {{name}}. It shows beside Claude, Codex and Cursor when you start an agent here.",
									{ name: name }
								)
							: translate("Connect to {{name}} to add one.", { name: name })
					}
				/>
			) : null}
			{agents.map((agent) =>
				editing?.id === agent.id ? null : (
					<AgentRow
						key={agent.id}
						agent={agent}
						view={view}
						live={live && !editing}
						note={note?.id === agent.id ? note.note : null}
						onEdit={() => {
							setNote(null);
							setEditing(draftOf(agent));
						}}
						onRemove={() => void remove(agent)}
					/>
				)
			)}
			{editing ? (
				<AgentForm
					key={editing.id ?? "new"}
					device={device}
					view={view}
					initial={editing}
					live={live}
					containers={containers}
					onClose={() => setEditing(null)}
				/>
			) : (
				<div className="flex justify-end border-t border-sidebar-border px-4 py-2.5 first:border-t-0">
					<button
						type="button"
						className={SECONDARY}
						disabled={!live}
						onClick={() => setEditing(draftOf())}
					>
						{translate("Add agent")}
					</button>
				</div>
			)}
		</Section>
	);
};

/** One agent: its command, what its last start found, Edit and Remove. */
const AgentRow = ({
	agent,
	view,
	live,
	note,
	onEdit,
	onRemove,
}: {
	agent: AcpAgent;
	view: ViewState;
	live: boolean;
	note: Note | null;
	onEdit: () => void;
	onRemove: () => void;
}) => {
	useI18n();
	const probe = probeLine(agent, accountOfAgent(view, agent));
	const command = [agent.program, ...(agent.args ?? [])].join(" ");
	return (
		<Row
			title={agent.name}
			sub={
				<span className="flex flex-col gap-0.5">
					<span className="flex min-w-0 items-center gap-0.5">
						<span className="min-w-0 truncate font-mono text-[11.5px]">{command}</span>
						<CopyIcon
							text={command}
							label={translate("Copy command")}
							className="size-[18px]"
						/>
					</span>
					<span className={cn(probe.error && "text-status-error-text")}>
						{probe.text}
						{probe.error ? <ErrorCopy text={probe.text} /> : null}
					</span>
				</span>
			}
		>
			<StatusNote note={note} />
			<button type="button" className={SECONDARY} disabled={!live} onClick={onEdit}>
				{translate("Edit")}
			</button>
			<ConfirmButton
				label={translate("Remove")}
				question={translate("Remove {{name}}?", { name: agent.name })}
				confirm={translate("Remove")}
				disabled={!live}
				onConfirm={onRemove}
			/>
		</Row>
	);
};

/** The form that adds an agent or changes one. */
const AgentForm = ({
	device,
	view,
	initial,
	live,
	containers,
	onClose,
}: {
	device: DeviceId;
	view: ViewState;
	initial: AcpDraft;
	live: boolean;
	/** Whether the device has containers its agents could run in. */
	containers: boolean;
	onClose: () => void;
}) => {
	useI18n();
	const { source } = useServices();
	const [draft, setDraft] = useState(initial);
	// A new agent's ID, made once so a second Save changes the same one.
	const [id] = useState(() => initial.id ?? crypto.randomUUID());
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	// The value fields by row key. Only Save reads them.
	const fields = useRef(new Map<string, HTMLInputElement>());
	const change = (part: Partial<AcpDraft>) => setDraft((d) => ({ ...d, ...part }));
	const changeVariable = (key: string, part: Partial<VariableRow>) =>
		setDraft((d) => ({ ...d, env: d.env.map((r) => (r.key === key ? { ...r, ...part } : r)) }));

	const save = async (e: FormEvent) => {
		e.preventDefault();
		if (busy) return;
		const typed = new Map<string, string>();
		for (const row of draft.env) {
			if (sendsValue(row)) typed.set(row.key, fields.current.get(row.key)?.value ?? "");
		}
		const plan = planAcpSave(view, device, draft, id, typed);
		if (!plan.ok) {
			setNote({ text: plan.reason, error: true });
			return;
		}
		// The values go once. Past here only the request holds them, so a
		// save that fails asks for them again.
		for (const field of fields.current.values()) field.value = "";
		const values = plan.env.map((v) => v.value);
		setBusy(true);
		setNote({ text: translate("Saving and starting it once to check it"), error: false });
		try {
			const failure = failureOf(await source.setAcpAgent(device, plan.agent, plan.env));
			if (failure) setNote({ text: sentence(scrubAll(failure, values)), error: true });
			else onClose();
		} catch (error) {
			const reason = error instanceof Error ? error.message : String(error);
			console.warn(translate("Couldn't save the ACP agent:"), scrubAll(reason, values));
			setNote({
				text: translate("Couldn't reach the device. Type the values again and save."),
				error: true,
			});
		} finally {
			setBusy(false);
		}
	};

	const off = !live || busy;
	return (
		<form
			aria-label={
				initial.id
					? translate("Change {{name}}", { name: initial.name })
					: translate("New ACP agent")
			}
			onSubmit={(e) => void save(e)}
			className="flex flex-col gap-3.5 border-t border-sidebar-border px-4 py-3.5 first:border-t-0"
		>
			<div className="grid grid-cols-2 gap-3">
				<label className="flex flex-col gap-1.5">
					<Label>{translate("Name")}</Label>
					<input
						aria-label={translate("Name")}
						value={draft.name}
						placeholder="Gemini"
						onChange={(e) => change({ name: e.target.value })}
						disabled={off}
						autoComplete="off"
						spellCheck={false}
						className={FIELD}
					/>
				</label>
				<label className="flex flex-col gap-1.5">
					<Label>{translate("Program")}</Label>
					<input
						aria-label={translate("Program")}
						value={draft.program}
						placeholder={translate("A name on PATH, or a full path")}
						onChange={(e) => change({ program: e.target.value })}
						disabled={off}
						autoComplete="off"
						spellCheck={false}
						autoCorrect="off"
						autoCapitalize="off"
						className={cn(FIELD, "font-mono text-xs")}
					/>
				</label>
			</div>

			<div role="group" aria-label={translate("Arguments")} className="flex flex-col gap-1.5">
				<Label>{translate("Arguments")}</Label>
				<span className="text-xs text-muted-foreground">
					{translate("One per row, passed as typed, with no shell in between.")}
				</span>
				{draft.args.map((row, i) => (
					<div key={row.key} className="flex items-center gap-2">
						<input
							aria-label={translate("Argument {{value1}}", { value1: i + 1 })}
							value={row.value}
							onChange={(e) =>
								change({
									args: draft.args.map((a) =>
										a.key === row.key ? { ...a, value: e.target.value } : a
									),
								})
							}
							disabled={off}
							autoComplete="off"
							spellCheck={false}
							autoCorrect="off"
							autoCapitalize="off"
							className={cn(FIELD, "font-mono text-xs")}
						/>
						<button
							type="button"
							aria-label={translate("Remove argument {{value1}}", { value1: i + 1 })}
							className={SECONDARY}
							disabled={off}
							onClick={() =>
								change({ args: draft.args.filter((a) => a.key !== row.key) })
							}
						>
							{translate("Remove")}
						</button>
					</div>
				))}
				<div>
					<button
						type="button"
						className={SECONDARY}
						disabled={off}
						onClick={() => change({ args: [...draft.args, newArgument()] })}
					>
						{translate("Add argument")}
					</button>
				</div>
			</div>

			<div role="group" aria-label={translate("Variables")} className="flex flex-col gap-1.5">
				<Label>{translate("Variables")}</Label>
				<span className="text-xs text-muted-foreground">
					{translate(
						"Set for the agent when it starts, such as an API key. The device keeps each value as a secret and never shows it again."
					)}
				</span>
				{draft.env.map((row) => (
					<VariableField
						key={row.key}
						row={row}
						disabled={off}
						field={(el) => {
							if (el) fields.current.set(row.key, el);
							else fields.current.delete(row.key);
						}}
						onName={(name) => changeVariable(row.key, { name })}
						onReplace={(replacing) => changeVariable(row.key, { replacing })}
						onRemove={() => change({ env: draft.env.filter((r) => r.key !== row.key) })}
					/>
				))}
				<div>
					<button
						type="button"
						className={SECONDARY}
						disabled={off}
						onClick={() => change({ env: [...draft.env, newVariable()] })}
					>
						{translate("Add variable")}
					</button>
				</div>
			</div>

			{containers ? (
				<label className="flex items-center justify-between gap-4">
					<span className="flex flex-col gap-0.5">
						<span className="text-[12.5px] text-foreground">
							{translate("Also run in containers")}
						</span>
						<span className="text-xs text-muted-foreground">
							{translate(
								"Agents of it may run in this device's workspace containers too. The program must be installed in each one, and you sign in to it there."
							)}
						</span>
					</span>
					<Switch
						aria-label={translate("Also run in containers")}
						checked={draft.containers}
						disabled={off}
						onCheckedChange={(on) => change({ containers: on })}
					/>
				</label>
			) : null}

			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} className="mr-auto" />
				<button type="button" className={SECONDARY} disabled={busy} onClick={onClose}>
					{translate("Cancel")}
				</button>
				<button type="submit" className={PRIMARY} disabled={off}>
					{busy ? translate("Saving") : translate("Save")}
				</button>
			</div>
		</form>
	);
};

/**
 * One variable. A value the device holds shows as set, with Replace and
 * Remove. A new one, or one being replaced, gets a password field.
 */
const VariableField = ({
	row,
	disabled,
	field,
	onName,
	onReplace,
	onRemove,
}: {
	row: VariableRow;
	disabled: boolean;
	field: (el: HTMLInputElement | null) => void;
	onName: (name: string) => void;
	onReplace: (replacing: boolean) => void;
	onRemove: () => void;
}) => {
	useI18n();
	const label = row.name.trim() || translate("the new variable");
	return (
		<div className="flex items-center gap-2">
			{row.held ? (
				<span className="w-[40%] shrink-0 truncate font-mono text-xs text-foreground">
					{row.name}
				</span>
			) : (
				<input
					aria-label={translate("Variable name")}
					value={row.name}
					placeholder="NAME"
					onChange={(e) => onName(e.target.value)}
					disabled={disabled}
					autoComplete="off"
					spellCheck={false}
					autoCorrect="off"
					autoCapitalize="off"
					className={cn(FIELD, "w-[40%] shrink-0 font-mono text-xs")}
				/>
			)}
			{sendsValue(row) ? (
				<input
					ref={field}
					type="password"
					aria-label={translate("Value of {{label}}", { label: label })}
					placeholder={translate("Value")}
					disabled={disabled}
					autoComplete="off"
					spellCheck={false}
					autoCorrect="off"
					autoCapitalize="off"
					className={cn(FIELD, "font-mono text-xs")}
				/>
			) : (
				<span
					aria-label={translate("{{name}} is set", { name: row.name })}
					className="flex-1 text-xs tracking-[0.2em] text-muted-foreground"
				>
					••••••••
				</span>
			)}
			{row.held ? (
				<button
					type="button"
					aria-label={
						row.replacing
							? translate("Keep {{name}}", { name: row.name })
							: translate("Replace {{name}}", { name: row.name })
					}
					className={SECONDARY}
					disabled={disabled}
					onClick={() => onReplace(!row.replacing)}
				>
					{row.replacing ? translate("Keep") : translate("Replace")}
				</button>
			) : null}
			<button
				type="button"
				aria-label={translate("Remove {{label}}", { label: label })}
				className={SECONDARY}
				disabled={disabled}
				onClick={onRemove}
			>
				{translate("Remove")}
			</button>
		</div>
	);
};
