// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The form that makes a container workspace for a project: the project, the
// device with Incus, the container's name and whether it starts at boot.
// Once the device takes it, the form follows the workspace step by step
// until it runs or fails, with the egress result and what answered.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { ConnectionDot } from "@/components/status/dots";
import { Switch } from "@/components/ui/switch";
import type { DeviceId, ProjectId, WorkspaceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { Picker } from "@/new-agent/picker";
import { FIELD, Label, PRIMARY, SECONDARY } from "@/settings/controls";
import { sendFirstSettings } from "@/settings/project";
import { failureOf } from "@/settings/result";
import { plural, sentence } from "@maxhq/shared/status/format";

import {
	creationSteps,
	madeInstance,
	madeOrFailed,
	type SeenSteps,
	seeStep,
	stepsLeft,
} from "@maxhq/shared/workspaces/creation";
import {
	defaultName,
	egressText,
	guestText,
	hostChoices,
	isLive,
	makesWorkspaces,
	planWorkspace,
	projectChoices,
	STATE_TONE,
	stateLook,
	stateText,
	type WorkspaceChoices,
	workspaceHosts,
} from "@maxhq/shared/workspaces/state";

/** A dialog's title, as the workspace forms set it. */
export const Title = ({ children }: { children: string }) => {
	useI18n();
	return (
		<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
			{children}
		</Dialog.Title>
	);
};

/** The dimmed page behind a workspace form. */
export const BACKDROP =
	"fixed inset-0 z-[130] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0";

/** A workspace form's box. */
export const POPUP =
	"fixed top-[12vh] left-1/2 z-[131] w-[min(520px,calc(100vw-32px))] -translate-x-1/2 rounded-xl border border-input bg-card p-5 shadow-popup outline-none";

/** A workspace being made, step by step, then its state, guest and egress. */
export const CreationSteps = ({ id, planned }: { id: WorkspaceId; planned: string }) => {
	const { t } = useI18n();

	const workspace = useView((s) => s.view.tables.workspace.get(id));
	const now = useNow(5000);
	// The device names the container, so its name wins over the one planned.
	const instance = madeInstance(workspace, planned);
	const managed = workspace?.managed ?? undefined;
	const state = managed?.state;
	// The device names only the step it's on, so the steps already seen are
	// kept here by their place.
	const [seen, setSeen] = useState<SeenSteps>(() => new Map());
	const next = seeStep(seen, state);
	if (next !== seen) setSeen(next);
	const left = stepsLeft(state);

	return (
		<>
			<ol aria-label={t("Steps")} className="flex flex-col gap-2">
				{creationSteps(next, state).map((step) => (
					<li
						key={step.at}
						className="flex items-center gap-2.5 text-[12.5px] text-foreground"
					>
						<ConnectionDot size={8} look={step.look} label={step.label} />
						<span className="selectable">{step.text}</span>
					</li>
				))}
				{left > 0 ? (
					<li className="pl-[18px] text-xs text-muted-foreground">
						{plural(left, "more step")}
					</li>
				) : null}
			</ol>

			{!workspace || !managed ? (
				<p role="status" className="selectable text-xs text-muted-foreground">
					{t("{{instance}} is gone from the device.", { instance: instance })}
				</p>
			) : (
				<div role="status" className="flex flex-col gap-1 text-xs text-muted-foreground">
					<span
						className={cn(
							"selectable text-[12.5px]",
							STATE_TONE[stateLook(managed.state)]
						)}
					>
						{managed.state.state === "running"
							? t("{{instance}} is ready.", { instance: instance })
							: `${stateText(managed.state)}.`}
					</span>
					{managed.state.state === "creating" ? null : (
						<span className="selectable">{guestText(managed.guest, now)}</span>
					)}
					<span
						className={cn(
							"selectable",
							managed.egress.state === "failed" && "text-status-error-text"
						)}
					>
						{egressText(managed.egress, now)}
					</span>
				</div>
			)}
		</>
	);
};

/** The workspace being made, step by step, then how it ended. */
const Progress = ({ id, planned }: { id: WorkspaceId; planned: string }) => {
	const { t } = useI18n();

	const workspace = useView((s) => s.view.tables.workspace.get(id));
	const ended = madeOrFailed(workspace);
	return (
		<div className="flex flex-col gap-4">
			<div className="flex flex-col gap-1">
				<Title>
					{t("Making {{value1}}", { value1: madeInstance(workspace, planned) })}
				</Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{ended
						? t("The device has finished.")
						: t("It keeps going on the device if you close this.")}
				</Dialog.Description>
			</div>

			<CreationSteps id={id} planned={planned} />

			<div className="flex justify-end">
				<Dialog.Close className={ended ? PRIMARY : SECONDARY}>
					{ended ? t("Done") : t("Close")}
				</Dialog.Close>
			</div>
		</div>
	);
};

const Form = ({ project, device }: { project: ProjectId | null; device: DeviceId | null }) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const live = (id: DeviceId) => isLive(view, id, now);
	const [choices, setChoices] = useState<WorkspaceChoices>(() => ({
		project,
		device:
			device ??
			workspaceHosts(view).find((id) => live(id) && makesWorkspaces(view, id)) ??
			null,
		name: "",
		startAtBoot: true,
	}));
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);
	const [made, setMade] = useState<{ id: WorkspaceId; instance: string } | null>(null);

	if (made) return <Progress id={made.id} planned={made.instance} />;

	const plan = planWorkspace(view, choices, live);
	const chosen = choices.project ? view.tables.project.get(choices.project) : undefined;
	const set = (change: Partial<WorkspaceChoices>) => {
		setFailure(null);
		setChoices((c) => ({ ...c, ...change }));
	};

	const create = async () => {
		if (!plan.ok || busy) return;
		setBusy(true);
		setFailure(null);
		try {
			// Sent first, so the clone in the new container already commits
			// as this Mac's git identity.
			await sendFirstSettings(services, plan.spec.project, plan.spec.device);
			const result = await services.source.createWorkspace(plan.spec);
			if (result.status === "ok" && result.data.status === "created") {
				setMade({ id: result.data.workspace, instance: plan.instance });
			} else {
				setFailure(`${sentence(failureOf(result) ?? t("The device didn't make it"))}.`);
			}
		} catch (error) {
			console.warn("Couldn't create the workspace", error);
			setFailure(t("Couldn't reach the device. Try again."));
		} finally {
			setBusy(false);
		}
	};

	return (
		<form
			className="flex flex-col gap-4"
			onSubmit={(e) => {
				e.preventDefault();
				void create();
			}}
		>
			<div className="flex flex-col gap-1">
				<Title>{t("New workspace")}</Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{t(
						"A container on a device with Incus, with the project's repo cloned in it. MaxHQ checks what the container can reach before any agent starts there."
					)}
				</Dialog.Description>
			</div>

			<div className="flex flex-wrap gap-2">
				<Picker
					label={t("Project")}
					value={choices.project ?? ""}
					options={projectChoices(view)}
					onChange={(value) => set({ project: value })}
				/>
				<Picker
					label={t("Device")}
					value={choices.device ?? ""}
					options={hostChoices(view, live)}
					onChange={(value) => set({ device: value })}
				/>
			</div>

			<label className="flex flex-col gap-1.5">
				<Label>{t("Container name")}</Label>
				<span className="flex items-center gap-1.5">
					<span className="font-mono text-xs text-muted-foreground">mhq-</span>
					<input
						aria-label={t("Container name")}
						value={choices.name}
						onChange={(e) => set({ name: e.target.value })}
						placeholder={chosen ? defaultName(chosen.name) : "name"}
						spellCheck={false}
						autoComplete="off"
						className={cn(FIELD, "font-mono text-xs")}
					/>
				</span>
			</label>

			<div className="flex items-center gap-3">
				<div className="flex min-w-0 flex-1 flex-col gap-0.5">
					<span className="text-[12.5px] text-foreground">{t("Start at boot")}</span>
					<span className="text-xs text-muted-foreground">
						{t("Starts the container whenever the device boots.")}
					</span>
				</div>
				<Switch
					aria-label={t("Start at boot")}
					checked={choices.startAtBoot}
					onCheckedChange={(on) => set({ startAtBoot: on })}
				/>
			</div>

			{failure || !plan.ok ? (
				<p
					role="status"
					className={cn(
						"selectable text-[11.5px]",
						failure ? "text-status-error-text" : "text-muted-foreground"
					)}
				>
					{failure ?? (plan.ok ? null : plan.reason)}
				</p>
			) : null}

			<div className="flex items-center justify-end gap-2">
				<Dialog.Close className={SECONDARY}>{t("Cancel")}</Dialog.Close>
				<button type="submit" className={PRIMARY} disabled={!plan.ok || busy}>
					{busy ? t("Creating") : t("Create")}
				</button>
			</div>
		</form>
	);
};

/** The New workspace button and its form, for a project or a device. */
export const NewWorkspaceButton = ({
	project = null,
	device = null,
	disabled,
}: {
	project?: ProjectId | null;
	device?: DeviceId | null;
	disabled?: boolean;
}) => {
	const { t } = useI18n();

	const [open, setOpen] = useState(false);
	const [opened, setOpened] = useState(0);
	return (
		<Dialog.Root open={open} onOpenChange={setOpen}>
			<button
				type="button"
				className={SECONDARY}
				disabled={disabled}
				onClick={() => {
					setOpened((n) => n + 1);
					setOpen(true);
				}}
			>
				{t("New workspace")}
			</button>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={POPUP}>
					<Form key={opened} project={project} device={device} />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
