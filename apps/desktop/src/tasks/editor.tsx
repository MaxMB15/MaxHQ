// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sheet that makes or edits a scheduled task: its name and prompt,
// what makes it run, the agent it reaches and the device it lives on. A
// new webhook's URL shows once, after the save, and a signing secret goes
// to the device on its own call, never into the task.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { useFieldTyping } from "@/composer/typing-prefs";
import { cn } from "@/lib/utils";
import { AGENT_FIELD, AgentPickers } from "@/new-agent/agent-pickers";
import { Picker } from "@/new-agent/picker";
import { ConfirmButton, Label, PRIMARY, SECONDARY, StatusNote } from "@/settings/controls";
import type { Note } from "@/settings/result";
import type { DeviceId, Task } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { resolve } from "@maxhq/shared/new-agent/spec";
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
} from "@maxhq/shared/tasks/place";
import { saveDraft } from "@maxhq/shared/tasks/actions";
import { deviceName } from "@maxhq/shared/view/select";

import { useTaskActions } from "./actions";
import {
	AddressNote,
	GithubFields,
	HoldField,
	HookUrl,
	Part,
	PromptFields,
	RulesFields,
	SignatureFields,
	TextField,
} from "./hook-fields";

const Form = ({
	saved,
	savedHome,
	onDone,
}: {
	saved: Task | null;
	savedHome: DeviceId | null;
	onDone: () => void;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const actions = useTaskActions();
	const typing = useFieldTyping();
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
	// A webhook URL, shown once after a save or a rotate.
	const [url, setUrl] = useState<string | null>(null);
	const [made, setMade] = useState(false);

	const r = resolve(view, draft.choices);
	const home = picked ?? draftHome(view, draft);
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
			const saving = await saveDraft(source, view, draft, home, listen, crypto.randomUUID());
			if (saving.state === "refused") return setNote({ text: saving.reason, error: true });
			if (saving.state === "partly") {
				setDraft((d) => ({ ...d, saved: saving.task, secret: "" }));
				return setNote({ text: saving.reason, error: true });
			}
			if (saving.hookUrl) {
				setUrl(saving.hookUrl);
				setMade(true);
				return;
			}
			onDone();
		} finally {
			setBusy(false);
		}
	};

	const rotate = async () => {
		if (!saved) return;
		const failure = await actions.rotate(saved, setUrl);
		if (failure) setNote({ text: failure, error: true });
	};

	const remove = async () => {
		if (!saved) return;
		const failure = await actions.remove(saved);
		if (failure) return setNote({ text: failure, error: true });
		onDone();
	};

	if (made && url) {
		return (
			<div className="flex flex-col gap-4">
				<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
					{t("Webhook made")}
				</Dialog.Title>
				<HookUrl url={url} />
				{reach && home ? (
					<AddressNote reach={reach} device={deviceName(view, home)} />
				) : null}
				<div className="flex justify-end">
					<button type="button" className={PRIMARY} onClick={onDone}>
						{t("Done")}
					</button>
				</div>
			</div>
		);
	}

	const requests = takesRequests(draft.kind);

	return (
		<form
			aria-label={saved ? t("Edit task") : t("New task")}
			className="flex flex-col gap-4"
			onSubmit={(e) => {
				e.preventDefault();
				void save();
			}}
		>
			<div className="flex flex-col gap-1">
				<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
					{saved ? t("Edit task") : t("New scheduled task")}
				</Dialog.Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{t(
						"Its device sends the prompt to an agent when the task runs, with the Mac closed or not."
					)}
				</Dialog.Description>
			</div>

			<TextField label={t("Name")} value={draft.name} onChange={(name) => edit({ name })} />
			<label className="flex flex-col gap-1.5">
				<Label>{t("Prompt")}</Label>
				<textarea
					aria-label={t("Prompt")}
					{...typing}
					value={draft.prompt}
					onChange={(e) => edit({ prompt: e.target.value })}
					placeholder={t("What should the agent do each time this runs?")}
					rows={4}
					className={cn(AGENT_FIELD, "resize-none py-2 leading-normal")}
				/>
				{requests ? (
					<span className="text-[11.5px] text-muted-foreground">
						{t("Placeholders such as {{value2}} take the request's values.", {
							value2: "{{body.action}}",
						})}
					</span>
				) : null}
			</label>

			<Part title={t("When it runs")}>
				<Picker
					label={t("Runs")}
					value={draft.kind}
					options={TRIGGER_KINDS.map((t) => ({ value: t.kind, label: t.label }))}
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
						className="w-[160px]"
					/>
				) : null}
				{draft.kind === "at" ? (
					<>
						<div className="flex flex-wrap items-end gap-3">
							<TextField
								label={t("Time")}
								type="time"
								value={draft.time}
								onChange={(time) => edit({ time })}
								className="w-[120px]"
							/>
							<div role="group" aria-label={t("Days")} className="flex gap-1">
								{WEEKDAYS.map(({ day, label }) => {
									const on = draft.days.has(day);
									return (
										<button
											key={day}
											type="button"
											aria-pressed={on}
											onClick={() =>
												edit({ days: toggled(draft.days, day, !on) })
											}
											className={cn(
												"h-8 w-[42px] rounded-[7px] border text-[12px]",
												on
													? "border-primary bg-primary/15 text-foreground"
													: "border-input bg-secondary text-muted-foreground hover:bg-accent"
											)}
										>
											{label}
										</button>
									);
								})}
							</div>
						</div>
						<span className="text-[11.5px] text-muted-foreground">
							{t("In {{zone}}. No days picked runs it every day.", {
								zone: draft.zone,
							})}
						</span>
					</>
				) : null}
				{draft.kind === "github_events" ? (
					<GithubFields draft={draft} edit={edit} repos={repos} />
				) : null}
			</Part>

			{draft.kind === "hook" ? (
				<Part title={t("Webhook")}>
					<div className="flex items-center justify-between gap-3">
						<span className="flex flex-col gap-0.5">
							<span className="text-[12.5px] text-foreground">
								{t("Take requests")}
							</span>
							<span className="text-[11.5px] text-muted-foreground">
								{t(
									"Off turns requests away, and closes its Funnel path when it has one."
								)}
							</span>
						</span>
						<Switch
							aria-label={t("Take requests")}
							checked={listen}
							onCheckedChange={setListen}
						/>
					</div>
					{url ? <HookUrl url={url} /> : null}
					{saved && savedHook ? (
						<div className="flex items-center justify-between gap-3">
							<span className="text-[11.5px] text-muted-foreground">
								{t(
									"The URL showed once, when it was made. Rotate to get a new one."
								)}
							</span>
							<ConfirmButton
								label={t("Rotate URL")}
								question={t("The current URL stops working.")}
								confirm={t("Rotate")}
								onConfirm={() => void rotate()}
							/>
						</div>
					) : (
						<span className="text-[11.5px] text-muted-foreground">
							{t("The URL shows once, after you save.")}
						</span>
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
				<div className="flex flex-wrap gap-2">
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
				</div>
				{warning ? (
					<span className="text-[11.5px] text-status-busy-text">{warning}</span>
				) : null}
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

			<div className="flex items-center justify-end gap-2.5">
				{saved ? (
					<ConfirmButton
						label={t("Delete")}
						question={t("Its runs and requests go too.")}
						confirm={t("Delete")}
						onConfirm={() => void remove()}
						disabled={busy}
					/>
				) : null}
				<StatusNote note={note} className="min-w-0 flex-1 text-right" />
				<Dialog.Close className={SECONDARY} disabled={busy}>
					{t("Cancel")}
				</Dialog.Close>
				<button type="submit" className={PRIMARY} disabled={busy}>
					{saved ? t("Save task") : t("Make task")}
				</button>
			</div>
		</form>
	);
};

/** What the editor is open on: a saved task and its home, or a new task. */
export type Editing = { readonly task: Task | null; readonly home: DeviceId | null };

/** The editor sheet, open while `editing` isn't null. */
export const TaskEditor = ({
	editing,
	onClose,
}: {
	editing: Editing | null;
	onClose: () => void;
}) => {
	useI18n();
	return (
		<Dialog.Root
			open={editing !== null}
			onOpenChange={(next) => {
				if (!next) onClose();
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-1/2 left-1/2 z-[121] max-h-[calc(100vh-64px)] w-[min(600px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					{editing ? (
						<Form
							key={editing.task?.id ?? "new"}
							saved={editing.task}
							savedHome={editing.home}
							onDone={onClose}
						/>
					) : null}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
