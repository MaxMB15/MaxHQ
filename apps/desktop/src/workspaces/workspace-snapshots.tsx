// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A container workspace's snapshots on its card: the list with Refresh, and
// for each one Restore as new, which changes nothing that's there, and Roll
// back, which asks first. A roll back over unsaved work says what it would
// lose and takes a second click to throw that away.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import type { Timestamp, Workspace } from "@maxhq/shared/bindings";
import {
	refreshSnapshots,
	restoreSnapshot,
	snapshotLine,
	useSnapshots,
} from "@maxhq/shared/hosts/snapshots";
import { rollbackLoses } from "@maxhq/shared/hosts/unsaved";
import { DANGER, SECONDARY, StatusNote } from "@/settings/controls";
import type { Note } from "@/settings/result";
import { sentence } from "@maxhq/shared/status/format";

/** The snapshots of a container workspace, with restore and roll back. */
export const WorkspaceSnapshots = ({
	workspace,
	instance,
	live,
	now,
}: {
	workspace: Workspace;
	/** The container's name. */
	instance: string;
	/** Whether its device answers, so a restore can go. */
	live: boolean;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const list = useSnapshots(source, workspace.id);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const [rolling, setRolling] = useState<string | null>(null);

	const restore = async (snapshot: string, asNew: boolean, discardUnsaved: boolean) => {
		setBusy(true);
		setRolling(null);
		setNote({ text: asNew ? t("Making a new workspace") : t("Rolling back"), error: false });
		try {
			await restoreSnapshot(source, workspace.id, snapshot, asNew, discardUnsaved);
			setNote({
				text: asNew
					? t("Made a new workspace from {{snapshot}}", { snapshot: snapshot })
					: t("Rolled back to {{snapshot}}", { snapshot: snapshot }),
				error: false,
			});
		} catch (error) {
			setNote({
				text: error instanceof Error ? sentence(error.message) : t("Couldn't restore it."),
				error: true,
			});
		} finally {
			setBusy(false);
		}
	};

	const refresh = async () => {
		setNote(null);
		await refreshSnapshots(source, workspace.id);
	};

	const snapshots = list?.snapshots ?? [];
	const loses = rollbackLoses(workspace, false);

	return (
		<div
			role="group"
			aria-label={t("Snapshots of {{instance}}", { instance: instance })}
			className="ml-[18px] flex flex-col rounded-lg border border-input bg-subpanel"
		>
			<div className="flex items-center gap-2 px-3 py-2">
				<span className="text-xs font-[600] text-secondary-foreground">
					{t("Snapshots")}
				</span>
				<span className="flex-1" />
				<StatusNote note={note} />
				<button
					type="button"
					className={SECONDARY}
					disabled={busy || list?.state === "loading"}
					onClick={() => void refresh()}
				>
					{t("Refresh")}
				</button>
			</div>
			{list?.state === "failed" ? (
				<p role="alert" className="px-3 pb-2 text-xs text-status-error-text">
					{sentence(list.error)}
				</p>
			) : null}
			{list === undefined || (list.state === "loading" && list.snapshots === null) ? (
				<p className="px-3 pb-2 text-xs text-muted-foreground">{t("Listing snapshots")}</p>
			) : snapshots.length === 0 ? (
				<p className="px-3 pb-2 text-xs text-muted-foreground">{t("No snapshots yet.")}</p>
			) : (
				<ul className="flex flex-col">
					{snapshots.map((snapshot) => (
						<li
							key={snapshot.name}
							aria-label={t("Snapshot {{name}}", { name: snapshot.name })}
							className="flex flex-col gap-2 border-t border-sidebar-border px-3 py-2"
						>
							<div className="flex items-center gap-2">
								<span className="flex min-w-0 flex-1 flex-col">
									<span className="truncate font-mono text-[12px] text-foreground">
										{snapshot.name}
									</span>
									<span className="text-[11.5px] text-muted-foreground">
										{snapshotLine(snapshot, now)}
									</span>
								</span>
								<button
									type="button"
									className={SECONDARY}
									disabled={!live || busy}
									aria-label={t("Restore {{name}} as a new workspace", {
										name: snapshot.name,
									})}
									onClick={() => void restore(snapshot.name, true, false)}
								>
									{t("Restore as new")}
								</button>
								<button
									type="button"
									className={SECONDARY}
									disabled={!live || busy || rolling === snapshot.name}
									aria-label={t("Roll {{instance}} back to {{name}}", {
										instance: instance,
										name: snapshot.name,
									})}
									onClick={() => setRolling(snapshot.name)}
								>
									{t("Roll back")}
								</button>
							</div>
							{rolling === snapshot.name ? (
								<div
									role={loses ? "alert" : "group"}
									aria-label={t("Roll back to {{name}}", { name: snapshot.name })}
									className={
										loses
											? "flex flex-col gap-2 rounded-lg border border-status-error/45 bg-status-error/10 px-3 py-2.5"
											: "flex flex-col gap-2 rounded-lg border border-input bg-card px-3 py-2.5"
									}
								>
									{loses ? (
										<span className="text-xs font-[600] text-status-error-text">
											{t("Rolling back loses this: {{loses}}.", {
												loses: loses,
											})}
										</span>
									) : null}
									<span className="text-xs text-secondary-foreground">
										{t(
											"{{instance}} stops, goes back to {{name}} and starts again. MaxHQ snapshots how it is now first.",
											{ instance: instance, name: snapshot.name }
										)}
										{loses
											? t(
													" Push your work first, or restore as new to keep both."
												)
											: ""}
									</span>
									<div className="flex items-center justify-end gap-2">
										<button
											type="button"
											className={SECONDARY}
											onClick={() => setRolling(null)}
										>
											{t("Keep it")}
										</button>
										<button
											type="button"
											className={DANGER}
											disabled={busy}
											onClick={() =>
												void restore(snapshot.name, false, loses !== null)
											}
										>
											{loses ? t("Discard and roll back") : t("Roll back")}
										</button>
									</div>
								</div>
							) : null}
						</li>
					))}
				</ul>
			)}
		</div>
	);
};
