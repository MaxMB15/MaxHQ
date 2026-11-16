// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One container workspace MaxHQ made: its dot and state, the guest and
// egress lines, its unsaved work, a bar while it's being made, Start, Stop
// and Delete, its snapshots, and a More menu that opens it in an editor.
// Delete asks once, and when the device refuses because of unpushed work it
// says so and takes a second click to throw that work away.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { OpenInMenu } from "@/agent-view/open-in";
import { useServices, useView } from "@/app/services";
import { ConnectionDot } from "@/components/status/dots";
import type { ActionResult } from "@maxhq/shared/data/source";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { DANGER, SECONDARY, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import { unsavedOf } from "@maxhq/shared/hosts/unsaved";
import { sentence } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import { bootLine, deleteRefusal, showsSnapshots } from "@maxhq/shared/workspaces/device";
import {
	ACTION_LABEL,
	ACTION_NOTE,
	cloneLabel,
	cloneOf,
	egressText,
	guestText,
	isLive,
	type MadeWorkspace,
	STATE_TONE,
	stateLook,
	stateText,
	type WorkspaceAction,
	workspaceActions,
} from "@maxhq/shared/workspaces/state";

import { WorkspaceSnapshots } from "./workspace-snapshots";

// Where the delete stands: not asked, asked once, or refused for work
// that isn't pushed, with the device's reason.
type Deleting =
	| { readonly step: "idle" }
	| { readonly step: "confirm" }
	| { readonly step: "unpushed"; readonly reason: string };

/** A workspace MaxHQ made, with what you can do to it. */
export const WorkspaceCard = ({
	workspace,
	showProject = true,
	showDevice = false,
}: {
	workspace: MadeWorkspace;
	/** Names the project, for lists that hold more than one. */
	showProject?: boolean;
	/** Names the device, for lists that span devices. */
	showDevice?: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const [deleting, setDeleting] = useState<Deleting>({ step: "idle" });
	const [showSnapshots, setShowSnapshots] = useState(false);

	const { managed, location } = workspace;
	const look = stateLook(managed.state);
	const state = cloneLabel(cloneOf(workspace)) ?? stateText(managed.state);
	const live = isLive(view, workspace.device, now);
	const where = deviceName(view, workspace.device);
	const project = workspace.project
		? (view.tables.project.get(workspace.project)?.name ?? workspace.project)
		: null;
	const actions = live ? workspaceActions(managed.state) : [];
	const unsaved = unsavedOf(workspace, now);
	const snapshots = showsSnapshots(view, workspace, actions);

	const run = async (action: WorkspaceAction, call: () => Promise<ActionResult>) => {
		setBusy(true);
		setNote({ text: ACTION_NOTE[action], error: false });
		try {
			const result = await call();
			const refused = action === "delete" ? deleteRefusal(result) : null;
			if (refused !== null) {
				setDeleting({ step: "unpushed", reason: refused });
				setNote(null);
				return;
			}
			const failure = failureOf(result);
			setNote(failure ? { text: sentence(failure), error: true } : null);
			setDeleting({ step: "idle" });
		} catch (error) {
			console.warn(`Couldn't ${action} the workspace`, error);
			setNote({ text: t("Couldn't reach the device. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	const press = (action: WorkspaceAction) => {
		switch (action) {
			case "start":
				return void run("start", () => source.startWorkspace(workspace.id));
			case "stop":
				return void run("stop", () => source.stopWorkspace(workspace.id));
			case "delete":
				return setDeleting({ step: "confirm" });
		}
	};

	const meta = [
		showProject ? project : null,
		showDevice ? t("on {{where}}", { where: where }) : null,
	]
		.filter(Boolean)
		.join(" ");

	return (
		<div
			role="group"
			aria-label={t("Workspace {{instance}}", { instance: location.instance })}
			className="flex flex-col gap-2 border-t border-sidebar-border px-4 py-3 first:border-t-0"
		>
			<div className="flex items-center gap-2.5">
				<ConnectionDot look={look} size={8} label={state} />
				<span className="min-w-0 truncate font-mono text-[12.5px] text-foreground-strong">
					{location.instance}
				</span>
				{meta ? (
					<span className="min-w-0 truncate text-[11.5px] text-muted-foreground">
						{meta}
					</span>
				) : null}
				<span className="flex-1" />
				<StatusNote note={note} />
				{deleting.step === "idle"
					? actions.map((action) => (
							<button
								key={action}
								type="button"
								className={SECONDARY}
								disabled={busy}
								onClick={() => press(action)}
							>
								{ACTION_LABEL[action]}
							</button>
						))
					: null}
				{snapshots ? (
					<button
						type="button"
						className={SECONDARY}
						aria-expanded={showSnapshots}
						onClick={() => setShowSnapshots((on) => !on)}
					>
						{t("Snapshots")}
					</button>
				) : null}
				<OpenInMenu
					workspace={workspace.id}
					name={location.instance}
					onFail={(text) => setNote({ text, error: true })}
				/>
			</div>

			<p className={cn("pl-[18px] text-xs", STATE_TONE[look])}>{state}</p>

			{managed.state.state === "creating" ? (
				<div
					role="progressbar"
					aria-label={t("Making {{instance}}", { instance: location.instance })}
					aria-valuemin={0}
					aria-valuemax={managed.state.of}
					aria-valuenow={managed.state.done}
					className="ml-[18px] h-1 overflow-hidden rounded-full bg-track"
				>
					<div
						className="h-full rounded-full bg-status-connecting transition-[width] duration-300"
						style={{
							width: `${(managed.state.done / Math.max(1, managed.state.of)) * 100}%`,
						}}
					/>
				</div>
			) : null}

			<ul className="flex flex-col gap-0.5 pl-[18px] text-xs text-muted-foreground">
				{managed.state.state === "creating" ? null : (
					<li>{guestText(managed.guest, now)}</li>
				)}
				<li className={cn(managed.egress.state === "failed" && "text-status-error-text")}>
					{egressText(managed.egress, now)}
				</li>
				<li>{bootLine(workspace, where)}</li>
				{unsaved ? (
					<li className={cn(unsaved.any && !unsaved.old && "text-secondary-foreground")}>
						{`${unsaved.text}. ${unsaved.readAt}.`}
					</li>
				) : null}
				<li className="font-mono text-[11.5px]">{location.path}</li>
			</ul>

			{snapshots && showSnapshots ? (
				<WorkspaceSnapshots
					workspace={workspace}
					instance={location.instance}
					live={live}
					now={now}
				/>
			) : null}

			{!live && workspaceActions(managed.state).length > 0 ? (
				<p className="pl-[18px] text-xs text-muted-foreground">
					{t("Connect to {{where}} to start, stop or delete it.", { where: where })}
				</p>
			) : null}

			{deleting.step === "confirm" ? (
				<div
					role="group"
					aria-label={t("Delete {{instance}}", { instance: location.instance })}
					className="ml-[18px] flex flex-wrap items-center gap-2 rounded-lg border border-input bg-subpanel px-3 py-2"
				>
					<span className="min-w-0 flex-1 text-xs text-secondary-foreground">
						{t(
							"Delete {{instance}}? The container and everything in it go, and so do its agents.",
							{ instance: location.instance }
						)}
					</span>
					<button
						type="button"
						className={SECONDARY}
						disabled={busy}
						onClick={() => setDeleting({ step: "idle" })}
					>
						{t("Keep it")}
					</button>
					<button
						type="button"
						className={DANGER}
						disabled={busy}
						onClick={() =>
							void run("delete", () => source.deleteWorkspace(workspace.id, false))
						}
					>
						{t("Delete")}
					</button>
				</div>
			) : null}

			{deleting.step === "unpushed" ? (
				<div
					role="alert"
					className="ml-[18px] flex flex-col gap-2 rounded-lg border border-status-error/45 bg-status-error/10 px-3 py-2.5"
				>
					<span className="text-xs font-[600] text-status-error-text">
						{`${sentence(deleting.reason)}.`}
					</span>
					<span className="text-xs text-secondary-foreground">
						{t(
							"Push it first to keep it. Deleting now throws that work away for good."
						)}
					</span>
					<div className="flex items-center justify-end gap-2">
						<button
							type="button"
							className={SECONDARY}
							disabled={busy}
							onClick={() => setDeleting({ step: "idle" })}
						>
							{t("Keep it")}
						</button>
						<button
							type="button"
							className={DANGER}
							disabled={busy}
							onClick={() =>
								void run("delete", () => source.deleteWorkspace(workspace.id, true))
							}
						>
							{t("Discard and delete")}
						</button>
					</div>
				</div>
			) : null}
		</div>
	);
};
