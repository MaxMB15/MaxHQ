// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The devflow pack row of a project's Skills tab: the library source its
// checkouts install whole, or none, and where each checkout's copy
// stands, with a failed install's last log lines and a retry. Taking the
// pack out asks first, and names the files left for you to delete.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { cn } from "@/lib/utils";
import { FIELD, PRIMARY, SECONDARY } from "@/settings/controls";
import { BACKDROP, POPUP, Title } from "@/workspaces/new-workspace";
import type { CheckoutSkills, SkillLibrary, SkillLock } from "@maxhq/shared/bindings";
import { packHasUpdate, packSources, packStateText, withPack } from "@maxhq/shared/skills/pack";

const SMALL = cn(SECONDARY, "h-[26px] px-2 text-xs");

/** What devflow's installer writes that taking the pack out leaves, since
 * you may have changed it. */
export const PACK_LEFT_FILES = ["CLAUDE.md", "AGENTS.md", ".devflow/", ".mcp.json"] as const;

/** Asks before the pack comes out of the checkouts. */
const RemoveDialog = ({
	open,
	onOpenChange,
	onRemove,
}: {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	onRemove: () => void;
}) => {
	const { t } = useI18n();
	return (
		<Dialog.Root open={open} onOpenChange={onOpenChange}>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={cn(POPUP, "w-[min(460px,calc(100vw-32px))]")}>
					<div className="flex flex-col gap-3">
						<Title>{t("Remove the devflow pack")}</Title>
						<p className="text-[12px] text-secondary-foreground">
							{t(
								"This takes the pack, its links and its exclude lines out of each checkout. These stay, since you may have changed them. Delete them by hand if you don't want them:"
							)}
						</p>
						<ul
							aria-label={t("Left for you to delete")}
							className="flex flex-col gap-0.5 rounded-[7px] border border-border bg-subpanel px-2.5 py-2"
						>
							{PACK_LEFT_FILES.map((path) => (
								<li key={path} className="font-mono text-[11.5px] text-foreground">
									{path}
								</li>
							))}
						</ul>
						<div className="flex items-center justify-end gap-2.5">
							<Dialog.Close className={SECONDARY}>{t("Cancel")}</Dialog.Close>
							<button type="button" className={PRIMARY} onClick={onRemove}>
								{t("Remove")}
							</button>
						</div>
					</div>
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};

export const PackRow = ({
	library,
	lock,
	checkouts,
	busy,
	onSave,
	onRetry,
}: {
	library: SkillLibrary;
	lock: SkillLock;
	checkouts: readonly CheckoutSkills[];
	busy: boolean;
	onSave: (next: SkillLock, doing: string, done: string) => void;
	onRetry: () => void;
}) => {
	const { t } = useI18n();

	const [removing, setRemoving] = useState(false);
	const sources = packSources(library);
	const shown = checkouts.filter((checkout) => checkout.pack);
	if (sources.length === 0 && !lock.pack && shown.length === 0) return null;
	return (
		<div className="flex flex-col gap-2 border-t border-sidebar-border px-3 py-3">
			<div className="flex items-center gap-2">
				<span className="mr-auto text-xs text-secondary-foreground">
					{t("Devflow pack")}
				</span>
				{packHasUpdate(library, lock) ? (
					<button
						type="button"
						aria-label={t("Update the pack")}
						className={SMALL}
						disabled={busy}
						onClick={() =>
							onSave(
								withPack(library, lock, lock.pack!.source),
								t("Updating"),
								t("Updated")
							)
						}
					>
						{t("Update")}
					</button>
				) : null}
				<select
					aria-label={t("Devflow pack")}
					className={cn(FIELD, "w-auto")}
					value={lock.pack?.source ?? ""}
					disabled={busy}
					onChange={(e) => {
						if (!e.target.value && lock.pack) setRemoving(true);
						else
							onSave(
								withPack(library, lock, e.target.value || null),
								t("Saving"),
								t("Saved")
							);
					}}
				>
					<option value="">{t("None")}</option>
					{sources.map((source) => (
						<option key={source.id} value={source.id}>
							{source.name}
						</option>
					))}
				</select>
			</div>
			{shown.length > 0 ? (
				<ul aria-label={t("Pack in the checkouts")} className="flex flex-col gap-1.5">
					{shown.map((checkout) => (
						<li key={checkout.path} className="flex flex-col gap-1">
							<span className="truncate font-mono text-[11px] text-muted-foreground">
								{checkout.path}
							</span>
							<span
								className={cn(
									"text-xs",
									checkout.pack?.state === "failed"
										? "text-status-busy-text"
										: "text-secondary-foreground"
								)}
							>
								{packStateText(checkout)}
							</span>
							{checkout.pack?.state === "failed" ? (
								<>
									<pre className="max-h-32 overflow-auto rounded bg-muted px-2 py-1 font-mono text-[11px] whitespace-pre-wrap text-muted-foreground">
										{checkout.pack.lines.join("\n")}
									</pre>
									<button
										type="button"
										aria-label={t("Retry the pack in {{path}}", {
											path: checkout.path,
										})}
										className={cn(SMALL, "self-start")}
										disabled={busy}
										onClick={onRetry}
									>
										{t("Retry")}
									</button>
								</>
							) : null}
						</li>
					))}
				</ul>
			) : null}
			<RemoveDialog
				open={removing}
				onOpenChange={setRemoving}
				onRemove={() => {
					setRemoving(false);
					onSave(withPack(library, lock, null), t("Removing"), t("Removed"));
				}}
			/>
		</div>
	);
};
