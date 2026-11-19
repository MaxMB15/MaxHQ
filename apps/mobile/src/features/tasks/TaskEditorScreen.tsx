// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sheet that makes or edits a scheduled task, as the desktop's: its
// name and prompt, what makes it run, the agent it reaches and the device
// it lives on. A new webhook's URL shows once, after the save, and a
// signing secret goes to the device on its own call, never into the task.
// Saving needs the Mac's switch for this phone on the task's device, so
// Save stays in view, off, until the Mac turns it on. A new URL for a
// webhook comes from the Mac only.

import { useI18n } from "@maxhq/shared/i18n";

import { type ReactNode, useState } from "react";
import { Platform, ScrollView, StyleSheet, Text, View } from "react-native";

import type { DeviceId, Task, Weekday } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { dateAtTime, timeOfDay } from "@maxhq/shared/lib/zoned-time";
import { newUuid } from "@maxhq/shared/lib/uuid";
import { resolve } from "@maxhq/shared/new-agent/spec";
import { saveDraft } from "@maxhq/shared/tasks/actions";
import {
	emptyDraft,
	promptFor,
	takesRequests,
	type TaskDraft,
	taskToDraft,
	toggled,
	TRIGGER_KINDS,
	type TriggerKind,
	WEEKDAYS,
} from "@maxhq/shared/tasks/draft";
import {
	draftHome,
	githubRepos,
	homeOptions,
	sleepWarning,
	targetOptions,
	targetPatch,
	targetValue,
	taskById,
} from "@maxhq/shared/tasks/place";
import { deviceName } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { confirm } from "../../ui/choose";
import { DateTimeField } from "../../ui/DateTimeField";
import { Header } from "../../ui/Header";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { useTaskActions } from "./actions";
import { AgentPickers } from "./AgentPickers";
import { Chips, Hint, Part, TextField, Tick } from "./fields";
import { AddressNote, HookUrl } from "./hook";
import { Picker } from "./Picker";
import {
	GithubFields,
	HoldField,
	PromptFields,
	RulesFields,
	SignatureFields,
} from "./request-fields";

/** A line under the form: how the save is going, or why it didn't go through. */
type Note = { readonly text: string; readonly error: boolean };

const Form = ({
	saved,
	savedHome,
	onDone,
	header,
}: {
	saved: Task | null;
	savedHome: DeviceId | null;
	onDone: () => void;
	/** The header with `title`, its back saying Close once there is nothing to cancel. */
	header: (title: string, done?: boolean) => ReactNode;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const actions = useTaskActions();
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const [draft, setDraft] = useState<TaskDraft>(() =>
		saved ? taskToDraft(services.view.getState().view, saved) : emptyDraft()
	);
	// The home you picked, which wins over the default until the save.
	const [picked, setPicked] = useState<DeviceId | null>(savedHome);
	const savedHook = saved?.trigger.type === "hook" ? saved.trigger : null;
	const [listen, setListen] = useState(savedHook?.on ?? true);
	const [note, setNote] = useState<Note | null>(null);
	const [busy, setBusy] = useState(false);
	// A new webhook's URL, shown once after the save.
	const [made, setMade] = useState<string | null>(null);

	const r = resolve(view, draft.choices);
	const home = picked ?? draftHome(view, draft);
	const allowed = useLetsPhone(home);
	const blocked = home !== null && !allowed;
	const reach = home ? view.tables.device_tasks.get(home) : undefined;
	const warning = sleepWarning(view, draft.kind, home);
	const repos = [...new Set(githubRepos(view).map((g) => g.repo))];
	const hasSecret = savedHook?.signature?.has_secret === true;
	// The agent the saved task reaches, which stays a choice even when it's
	// one an agent picked and the list leaves out.
	const kept = saved?.target.type === "agent" ? saved.target.agent : null;

	const edit = (patch: Partial<TaskDraft>) => {
		setDraft((d) => ({ ...d, ...patch }));
		setNote(null);
	};

	const save = async () => {
		setBusy(true);
		setNote({ text: t("Saving"), error: false });
		try {
			const saving = await saveDraft(source, view, draft, home, listen, newUuid());
			if (saving.state === "refused") return setNote({ text: saving.reason, error: true });
			if (saving.state === "partly") {
				setDraft((d) => ({ ...d, saved: saving.task, secret: "" }));
				return setNote({ text: saving.reason, error: true });
			}
			if (saving.hookUrl) return setMade(saving.hookUrl);
			onDone();
		} finally {
			setBusy(false);
		}
	};

	const remove = async () => {
		if (!saved) return;
		const failure = await actions.remove(saved);
		if (failure) return setNote({ text: failure, error: true });
		onDone();
	};

	if (made) {
		return (
			<>
				{header(t("Webhook made"), true)}
				<ScrollView contentContainerStyle={styles.content}>
					<HookUrl url={made} />
					{reach && home ? (
						<AddressNote reach={reach} device={deviceName(view, home)} />
					) : null}
					<Button label={t("Done")} tone="primary" onPress={onDone} />
				</ScrollView>
			</>
		);
	}

	const requests = takesRequests(draft.kind);

	return (
		<>
			{header(saved ? t("Edit task") : t("New scheduled task"))}
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				keyboardDismissMode="interactive"
				automaticallyAdjustKeyboardInsets
			>
				<Hint>
					{t(
						"Its device sends the prompt to an agent when the task runs, with the Mac closed or not."
					)}
				</Hint>

				<Part label={saved ? t("Edit task") : t("New task")}>
					<TextField
						label={t("Name")}
						value={draft.name}
						onChange={(name) => edit({ name })}
					/>
					<TextField
						label={t("Prompt")}
						value={draft.prompt}
						onChange={(prompt) => edit({ prompt })}
						placeholder={t("What should the agent do each time this runs?")}
						lines={5}
					/>
					{requests ? (
						<Hint>
							{t("Placeholders such as {{value2}} take the request's values.", {
								value2: "{{body.action}}",
							})}
						</Hint>
					) : null}
				</Part>

				<Part title={t("When it runs")}>
					<Picker
						label={t("Runs")}
						value={draft.kind}
						options={TRIGGER_KINDS.map((k) => ({ value: k.kind, label: k.label }))}
						onChange={(value) => {
							const kind = value as TriggerKind;
							edit({ kind, prompt: promptFor(kind, draft.prompt) });
						}}
					/>
					{draft.kind === "every" ? (
						<TextField
							label={t("Every, minutes")}
							value={draft.minutes}
							onChange={(minutes) => edit({ minutes })}
							keyboardType="number-pad"
						/>
					) : null}
					{draft.kind === "at" ? (
						<>
							<DateTimeField
								label={t("Time")}
								mode="time"
								value={dateAtTime(draft.time, draft.zone, new Date(now))}
								zone={draft.zone}
								onChange={(picked) => edit({ time: timeOfDay(picked, draft.zone) })}
							/>
							<Chips
								label={t("Days")}
								chips={WEEKDAYS.map(({ day, label }) => ({
									key: day,
									label,
									on: draft.days.has(day),
								}))}
								onToggle={(day, on) =>
									edit({ days: toggled(draft.days, day as Weekday, on) })
								}
							/>
							<Hint>
								{t("In {{zone}}. No days picked runs it every day.", {
									zone: draft.zone,
								})}
							</Hint>
						</>
					) : null}
					{draft.kind === "github_events" ? (
						<GithubFields draft={draft} edit={edit} repos={repos} />
					) : null}
				</Part>

				{draft.kind === "hook" ? (
					<Part title={t("Webhook")}>
						<Tick
							label={t("Take requests")}
							note={t(
								"Off turns requests away, and closes its Funnel path when it has one."
							)}
							value={listen}
							onChange={setListen}
						/>
						{saved && savedHook ? (
							<View style={styles.rotate}>
								<View style={styles.grow}>
									<Hint>
										{t(
											"The URL showed once, when it was made. Rotate to get a new one."
										)}
									</Hint>
								</View>
								<Text style={styles.onMac}>{t("On the Mac")}</Text>
							</View>
						) : (
							<Hint>{t("The URL shows once, after you save.")}</Hint>
						)}
						{reach && home ? (
							<AddressNote reach={reach} device={deviceName(view, home)} />
						) : null}
						<SignatureFields draft={draft} edit={edit} hasSecret={hasSecret} />
						<HoldField draft={draft} edit={edit} />
					</Part>
				) : null}

				{requests ? (
					<Part title={t("Fields in the prompt")}>
						<PromptFields draft={draft} edit={edit} />
					</Part>
				) : null}

				{requests ? (
					<Part title={t("Requests it takes")}>
						<RulesFields draft={draft} edit={edit} />
					</Part>
				) : null}

				<Part title={t("Where it goes")}>
					<Picker
						label={t("Sends to")}
						value={targetValue(draft)}
						options={targetOptions(view, kept)}
						onChange={(value) => edit(targetPatch(value))}
					/>
					<Picker
						label={t("Lives on")}
						value={home ?? ""}
						options={homeOptions(view)}
						onChange={(device) => {
							setPicked(device);
							setNote(null);
						}}
						disabled={saved !== null}
					/>
					{warning ? <Hint warn>{warning}</Hint> : null}
					{draft.target === "new" ? (
						<AgentPickers
							view={view}
							choices={draft.choices}
							resolved={r}
							now={now}
							onPick={(patch) => edit({ choices: { ...draft.choices, ...patch } })}
						/>
					) : null}
				</Part>

				{note ? (
					<Text
						style={[styles.note, note.error && styles.failed]}
						accessibilityRole={note.error ? "alert" : undefined}
					>
						{note.text}
					</Text>
				) : null}
				<View style={styles.footer}>
					{saved ? (
						<Button
							label={t("Delete")}
							tone="danger"
							disabled={busy}
							onPress={() =>
								confirm(
									{
										title: saved.name,
										detail: t("Its runs and requests go too."),
									},
									t("Delete"),
									() => void remove()
								)
							}
						/>
					) : null}
					<View style={styles.grow}>
						<Button
							label={saved ? t("Save task") : t("Make task")}
							tone="primary"
							disabled={busy || blocked}
							onPress={() => void save()}
						/>
					</View>
				</View>
				{blocked ? <NeedsSwitch /> : null}
			</ScrollView>
		</>
	);
};

/** The task editor, on a task by its ID, or on a new one. */
export const TaskEditorScreen = ({ navigation, route }: ScreenProps<"TaskEditor">) => {
	const { t } = useI18n();

	const services = useServices();
	const id = route.params?.task;
	// The task as it was when the editor opened, as the desktop's sheet holds it.
	const [found] = useState(() =>
		id === undefined ? undefined : taskById(services.view.getState().view, id)
	);
	const close = () => navigation.goBack();
	const header = (title: string, done?: boolean) => (
		<Header
			title={title}
			sheet={Platform.OS === "ios"}
			back={{ label: done ? t("Close") : t("Cancel"), onPress: close, close: true }}
		/>
	);

	return (
		<View style={styles.screen}>
			{id !== undefined && !found ? (
				<>
					{header(t("Edit task"), true)}
					<Text style={[styles.note, styles.gone]}>{t("This task is gone.")}</Text>
				</>
			) : (
				<Form
					saved={found?.task ?? null}
					savedHome={found?.home ?? route.params?.home ?? null}
					onDone={close}
					header={header}
				/>
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	rotate: { flexDirection: "row", alignItems: "center", gap: 12 },
	grow: { flex: 1 },
	onMac: { color: color.mutedForeground, fontSize: size.meta, fontWeight: "600" },
	note: { color: color.mutedForeground, fontSize: size.small },
	failed: { color: color.errorText },
	gone: { padding: 16 },
	footer: { flexDirection: "row", alignItems: "center", gap: 10 },
});
