// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The ACP agents in a device's settings on the phone, as on the Mac: the
// agent programs set up to run there, each with what its last start found
// and how to sign in when one of its agents waits for that, and the form
// that adds or changes one. A variable's value is typed into a password
// field that only Save reads; Save sends it and clears the field, and the
// device never sends it back. Saving and removing are requests the device
// takes from the phone only with the Mac's switch on for it.

import { useI18n } from "@maxhq/shared/i18n";

import { type ComponentRef, useRef, useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { AcpAgent, DeviceId } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import { newUuid } from "@maxhq/shared/lib/uuid";
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
	scrubAll,
	sendsValue,
	signInCommand,
	type VariableRow,
	waitingForSignIn,
} from "@maxhq/shared/settings/acp-agents";
import { sentence } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";

import { useServices } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { Input } from "../../ui/Field";
import {
	Buttons,
	CardBody,
	CopyTextButton,
	Muted,
	type Note,
	NoteLine,
	type SectionProps,
} from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { GatedToggle } from "../test-hosts/parts";

/** How to sign in to an agent program one of its agents waits on. */
const SignInHint = ({ view, agent, name }: { view: ViewState; agent: AcpAgent; name: string }) => {
	const { t } = useI18n();
	const account = waitingForSignIn(view, agent);
	if (!account) return null;
	const command = signInCommand(agent);
	const methods = account.acp?.auth ?? [];
	return (
		<View style={styles.hint}>
			<Text style={[styles.text, styles.error]}>
				{t("{{label}} needs a sign-in", { label: agent.name })}
			</Text>
			<Text style={styles.meta}>
				{t(
					"Sign in from a terminal on {{where}} the way {{label}} asks, then try again. MaxHQ keeps no login for it.",
					{ where: name, label: agent.name }
				)}
			</Text>
			{methods.map((m) => (
				<Text key={m.id} style={styles.meta}>
					{m.description ? `${m.name}: ${m.description}` : m.name}
				</Text>
			))}
			{command ? (
				<Buttons>
					<CopyTextButton label={t("Copy command")} text={command} />
				</Buttons>
			) : null}
		</View>
	);
};

/** One agent: its command, what its last start found, Edit and Remove. */
const AgentRow = ({
	agent,
	view,
	name,
	first,
	off,
	note,
	onEdit,
	onRemove,
}: {
	agent: AcpAgent;
	view: ViewState;
	name: string;
	first: boolean;
	off: boolean;
	note: Note | null;
	onEdit: () => void;
	onRemove: () => void;
}) => {
	const { t } = useI18n();
	const probe = probeLine(agent, accountOfAgent(view, agent));
	const command = [agent.program, ...(agent.args ?? [])].join(" ");
	const remove = t("Remove");
	return (
		<CardBody first={first}>
			<Text style={styles.name}>{agent.name}</Text>
			<Text selectable numberOfLines={2} style={styles.command}>
				{command}
			</Text>
			<Text selectable style={[styles.meta, probe.error && styles.error]}>
				{probe.text}
			</Text>
			<SignInHint view={view} agent={agent} name={name} />
			<NoteLine note={note} />
			<Buttons>
				<CopyTextButton label={t("Copy command")} text={command} />
				<Button label={t("Edit")} small disabled={off} onPress={onEdit} />
				<Button
					label={remove}
					tone="danger"
					small
					disabled={off}
					onPress={() =>
						confirm(
							{ title: t("Remove {{name}}?", { name: agent.name }), detail: "" },
							remove,
							onRemove
						)
					}
				/>
			</Buttons>
		</CardBody>
	);
};

type Field = ComponentRef<typeof Input>;

/**
 * One variable. A value the device holds shows as set, with Replace and
 * Remove. A new one, or one being replaced, gets a password field.
 */
const VariableField = ({
	row,
	disabled,
	field,
	onValue,
	onName,
	onReplace,
	onRemove,
}: {
	row: VariableRow;
	disabled: boolean;
	field: (el: Field | null) => void;
	onValue: (value: string) => void;
	onName: (name: string) => void;
	onReplace: (replacing: boolean) => void;
	onRemove: () => void;
}) => {
	const { t } = useI18n();
	const label = row.name.trim() || t("the new variable");
	return (
		<View style={styles.variable}>
			{row.held ? (
				<Text style={styles.command}>{row.name}</Text>
			) : (
				<Input
					accessibilityLabel={t("Variable name")}
					value={row.name}
					placeholder="NAME"
					onChangeText={onName}
					editable={!disabled}
					autoCapitalize="characters"
					autoCorrect={false}
					autoComplete="off"
					spellCheck={false}
					style={styles.mono}
				/>
			)}
			{sendsValue(row) ? (
				<Input
					ref={field}
					secureTextEntry
					accessibilityLabel={t("Value of {{label}}", { label: label })}
					placeholder={t("Value")}
					onChangeText={onValue}
					editable={!disabled}
					autoCapitalize="none"
					autoCorrect={false}
					autoComplete="off"
					spellCheck={false}
					style={styles.mono}
				/>
			) : (
				<Text
					accessibilityLabel={t("{{name}} is set", { name: row.name })}
					style={styles.meta}
				>
					••••••••
				</Text>
			)}
			<Buttons>
				{row.held ? (
					<Button
						label={row.replacing ? t("Keep") : t("Replace")}
						hint={
							row.replacing
								? t("Keep {{name}}", { name: row.name })
								: t("Replace {{name}}", { name: row.name })
						}
						small
						disabled={disabled}
						onPress={() => onReplace(!row.replacing)}
					/>
				) : null}
				<Button
					label={t("Remove")}
					hint={t("Remove {{label}}", { label: label })}
					small
					disabled={disabled}
					onPress={onRemove}
				/>
			</Buttons>
		</View>
	);
};

/** The form that adds an agent or changes one. */
const AgentForm = ({
	device,
	view,
	initial,
	off: locked,
	containers,
	onClose,
}: {
	device: DeviceId;
	view: ViewState;
	initial: AcpDraft;
	/** Set while the device isn't live or doesn't take the phone's requests. */
	off: boolean;
	/** Whether the device has containers its agents could run in. */
	containers: boolean;
	onClose: () => void;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const [draft, setDraft] = useState(initial);
	// A new agent's ID, made once so a second Save changes the same one.
	const [id] = useState(() => initial.id ?? newUuid());
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	// What each value field holds, and the fields, by row key. Only Save reads them.
	const values = useRef(new Map<string, string>());
	const fields = useRef(new Map<string, Field>());
	const change = (part: Partial<AcpDraft>) => setDraft((d) => ({ ...d, ...part }));
	const changeVariable = (key: string, part: Partial<VariableRow>) =>
		setDraft((d) => ({ ...d, env: d.env.map((r) => (r.key === key ? { ...r, ...part } : r)) }));

	const save = async () => {
		if (busy) return;
		const typed = new Map<string, string>();
		for (const row of draft.env) {
			if (sendsValue(row)) typed.set(row.key, values.current.get(row.key) ?? "");
		}
		const plan = planAcpSave(view, device, draft, id, typed);
		if (!plan.ok) {
			setNote({ text: plan.reason, error: true });
			return;
		}
		// The values go once. Past here only the request holds them, so a
		// save that fails asks for them again.
		values.current.clear();
		for (const field of fields.current.values()) field.clear();
		const sent = plan.env.map((v) => v.value);
		setBusy(true);
		setNote({ text: t("Saving and starting it once to check it"), error: false });
		try {
			const failure = failureOf(await source.setAcpAgent(device, plan.agent, plan.env));
			if (failure) setNote({ text: sentence(scrubAll(failure, sent)), error: true });
			else onClose();
		} catch (error) {
			const reason = error instanceof Error ? error.message : String(error);
			console.warn(t("Couldn't save the ACP agent:"), scrubAll(reason, sent));
			setNote({
				text: t("Couldn't reach the device. Type the values again and save."),
				error: true,
			});
		} finally {
			setBusy(false);
		}
	};

	const off = locked || busy;
	return (
		<View
			accessibilityLabel={
				initial.id ? t("Change {{name}}", { name: initial.name }) : t("New ACP agent")
			}
			style={styles.form}
		>
			<View style={styles.group}>
				<Text style={styles.label}>{t("Name")}</Text>
				<Input
					accessibilityLabel={t("Name")}
					value={draft.name}
					placeholder="Gemini"
					onChangeText={(name) => change({ name })}
					editable={!off}
					autoCorrect={false}
					autoComplete="off"
					spellCheck={false}
				/>
			</View>
			<View style={styles.group}>
				<Text style={styles.label}>{t("Program")}</Text>
				<Input
					accessibilityLabel={t("Program")}
					value={draft.program}
					placeholder={t("A name on PATH, or a full path")}
					onChangeText={(program) => change({ program })}
					editable={!off}
					autoCapitalize="none"
					autoCorrect={false}
					autoComplete="off"
					spellCheck={false}
					style={styles.mono}
				/>
			</View>

			<View accessibilityLabel={t("Arguments")} style={styles.group}>
				<Text style={styles.label}>{t("Arguments")}</Text>
				<Muted>{t("One per row, passed as typed, with no shell in between.")}</Muted>
				{draft.args.map((row, i) => (
					<View key={row.key} style={styles.argument}>
						<View style={styles.grow}>
							<Input
								accessibilityLabel={t("Argument {{value1}}", { value1: i + 1 })}
								value={row.value}
								onChangeText={(value) =>
									change({
										args: draft.args.map((a) =>
											a.key === row.key ? { ...a, value } : a
										),
									})
								}
								editable={!off}
								autoCapitalize="none"
								autoCorrect={false}
								autoComplete="off"
								spellCheck={false}
								style={styles.mono}
							/>
						</View>
						<Button
							label={t("Remove")}
							hint={t("Remove argument {{value1}}", { value1: i + 1 })}
							small
							disabled={off}
							onPress={() =>
								change({ args: draft.args.filter((a) => a.key !== row.key) })
							}
						/>
					</View>
				))}
				<Buttons>
					<Button
						label={t("Add argument")}
						small
						disabled={off}
						onPress={() => change({ args: [...draft.args, newArgument()] })}
					/>
				</Buttons>
			</View>

			<View accessibilityLabel={t("Variables")} style={styles.group}>
				<Text style={styles.label}>{t("Variables")}</Text>
				<Muted>
					{t(
						"Set for the agent when it starts, such as an API key. The device keeps each value as a secret and never shows it again."
					)}
				</Muted>
				{draft.env.map((row) => (
					<VariableField
						key={row.key}
						row={row}
						disabled={off}
						field={(el) => {
							if (el) fields.current.set(row.key, el);
							else fields.current.delete(row.key);
						}}
						onValue={(value) => values.current.set(row.key, value)}
						onName={(name) => changeVariable(row.key, { name })}
						onReplace={(replacing) => {
							if (!replacing) values.current.delete(row.key);
							changeVariable(row.key, { replacing });
						}}
						onRemove={() => {
							values.current.delete(row.key);
							change({ env: draft.env.filter((r) => r.key !== row.key) });
						}}
					/>
				))}
				<Buttons>
					<Button
						label={t("Add variable")}
						small
						disabled={off}
						onPress={() => change({ env: [...draft.env, newVariable()] })}
					/>
				</Buttons>
			</View>

			{containers ? (
				<View style={styles.group}>
					<GatedToggle
						label={t("Also run in containers")}
						value={draft.containers}
						disabled={off}
						onChange={(on) => change({ containers: on })}
					/>
					<Muted>
						{t(
							"Agents of it may run in this device's workspace containers too. The program must be installed in each one, and you sign in to it there."
						)}
					</Muted>
				</View>
			) : null}

			<NoteLine note={note} />
			<Buttons>
				<Button label={t("Cancel")} small disabled={busy} onPress={onClose} />
				<Button
					label={busy ? t("Saving") : t("Save")}
					tone="primary"
					small
					disabled={off}
					onPress={() => void save()}
				/>
			</Buttons>
		</View>
	);
};

/** The ACP agents card, or nothing when the device's `maxd` doesn't run them. */
export const AcpAgents = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();
	const { source } = useServices();
	const lets = useLetsPhone(device);
	const [editing, setEditing] = useState<AcpDraft | null>(null);
	const [note, setNote] = useState<{ id: string; note: Note } | null>(null);
	if (!runsAcpAgents(view, device)) return null;
	const agents = acpAgentsOn(view, device);
	// Running agents in the device's containers needs Incus there.
	const containers = view.tables.device.get(device)?.workspace_host != null;
	const off = !live || !lets;

	const remove = async (agent: AcpAgent) => {
		setNote({ id: agent.id, note: { text: t("Removing"), error: false } });
		try {
			const failure = failureOf(await source.deleteAcpAgent(device, agent.id));
			setNote(
				failure ? { id: agent.id, note: { text: sentence(failure), error: true } } : null
			);
		} catch (error) {
			console.warn(t("Couldn't remove the ACP agent"), error);
			setNote({
				id: agent.id,
				note: { text: t("Couldn't remove it. Try again."), error: true },
			});
		}
	};

	return (
		<Card
			title={t("ACP agents")}
			note={t("Agent programs that speak the Agent Client Protocol")}
			label={t("ACP agents on {{name}}", { name: name })}
		>
			{agents.length === 0 && !editing ? (
				<CardBody first>
					<Text style={styles.name}>{t("No ACP agents yet")}</Text>
					<Muted>
						{live
							? t(
									"Add an agent program that runs on {{name}}. It shows beside Claude, Codex and Cursor when you start an agent here.",
									{ name: name }
								)
							: t("Connect to {{name}} to add one.", { name: name })}
					</Muted>
				</CardBody>
			) : null}
			{agents.map((agent, i) =>
				editing?.id === agent.id ? null : (
					<AgentRow
						key={agent.id}
						agent={agent}
						view={view}
						name={name}
						first={i === 0}
						off={off || editing !== null}
						note={note?.id === agent.id ? note.note : null}
						onEdit={() => {
							setNote(null);
							setEditing(draftOf(agent));
						}}
						onRemove={() => void remove(agent)}
					/>
				)
			)}
			<CardBody first={agents.length === 0 && editing !== null}>
				{editing ? (
					<AgentForm
						key={editing.id ?? "new"}
						device={device}
						view={view}
						initial={editing}
						off={off}
						containers={containers}
						onClose={() => setEditing(null)}
					/>
				) : (
					<Buttons>
						<Button
							label={t("Add agent")}
							small
							disabled={off}
							onPress={() => setEditing(draftOf())}
						/>
					</Buttons>
				)}
				{lets ? null : <NeedsSwitch />}
			</CardBody>
		</Card>
	);
};

const styles = StyleSheet.create({
	name: { color: color.foreground, fontSize: size.body, fontWeight: "600" },
	text: { fontSize: size.small },
	meta: { color: color.mutedForeground, fontSize: size.meta },
	error: { color: color.errorText },
	command: { fontFamily: font.mono, fontSize: size.meta, color: color.foreground },
	mono: { fontFamily: font.mono, fontSize: size.meta },
	label: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "600" },
	hint: { gap: 4 },
	form: { gap: 14 },
	group: { gap: 6 },
	argument: { flexDirection: "row", alignItems: "center", gap: 8 },
	grow: { flex: 1, minWidth: 0 },
	variable: { gap: 6 },
});
