// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The consent step for tidying a device's global skills: the device's plan
// in full, which folders move into the library, which repos join as
// sources, which links appear and which copies go to the backup, a pick
// for each skill whose copies differ, then a tick for the device and
// Apply. Apply sends the plan's hash and the picks, so the device does what
// you saw or nothing. A skill you don't pick a copy of stays where it is.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { PRIMARY, SECONDARY, StatusNote } from "@/settings/controls";
import { BACKDROP, POPUP, Title } from "@/workspaces/new-workspace";
import type { DeviceId, GlobalsConflict, GlobalsJoin, GlobalsPlan } from "@maxhq/shared/bindings";
import { readOf } from "@maxhq/shared/pulls/read";
import { deviceName } from "@maxhq/shared/view/select";

import { useSkillAction } from "./look";

const List = ({ title, paths }: { title: string; paths: readonly string[] }) => {
	useI18n();
	return paths.length > 0 ? (
		<section aria-label={title} className="flex flex-col gap-1">
			<h3 className="text-[11.5px] font-[600] text-secondary-foreground">{title}</h3>
			<ul className="flex flex-col gap-0.5 rounded-[7px] border border-border bg-subpanel px-2.5 py-2">
				{paths.map((path) => (
					<li key={path} className="truncate font-mono text-[11.5px] text-foreground">
						{path}
					</li>
				))}
			</ul>
		</section>
	) : null;
};

const Joins = ({ joins }: { joins: readonly GlobalsJoin[] }) => {
	const { t } = useI18n();
	return joins.length > 0 ? (
		<section aria-label={t("Join as sources")} className="flex flex-col gap-1">
			<h3 className="text-[11.5px] font-[600] text-secondary-foreground">
				{t("Join as sources")}
			</h3>
			<ul className="flex flex-col gap-1.5 rounded-[7px] border border-border bg-subpanel px-2.5 py-2">
				{joins.map((join) => (
					<li key={join.url} className="flex flex-col gap-0.5">
						<span className="text-[12px] text-foreground">
							{join.name}{" "}
							<span className="text-muted-foreground">
								{t("from {{url}}", { url: join.url })}
							</span>
						</span>
						{join.paths.map((path) => (
							<span
								key={path}
								className="truncate font-mono text-[11.5px] text-muted-foreground"
							>
								{path}
							</span>
						))}
					</li>
				))}
			</ul>
		</section>
	) : null;
};

const Conflicts = ({
	conflicts,
	picks,
	onPick,
}: {
	conflicts: readonly GlobalsConflict[];
	picks: Readonly<Record<string, string>>;
	onPick: (name: string, path: string) => void;
}) => {
	const { t } = useI18n();
	return conflicts.length > 0 ? (
		<section aria-label={t("Pick one copy")} className="flex flex-col gap-1">
			<h3 className="text-[11.5px] font-[600] text-secondary-foreground">
				{t("Pick one copy")}
			</h3>
			<p className="text-[11.5px] text-muted-foreground">
				{t(
					"These copies differ. The one you pick moves into the library and the rest go to the backup. Without a pick they stay where they are."
				)}
			</p>
			<ul className="flex flex-col gap-2 rounded-[7px] border border-border bg-subpanel px-2.5 py-2">
				{conflicts.map((conflict) => (
					<li key={conflict.name}>
						<fieldset className="flex flex-col gap-0.5">
							<legend className="text-[12px] text-foreground">{conflict.name}</legend>
							{conflict.paths.map((path) => (
								<label
									key={path}
									className="flex items-center gap-2 font-mono text-[11.5px] text-foreground"
								>
									<input
										type="radio"
										name={`pick-${conflict.name}`}
										checked={picks[conflict.name] === path}
										onChange={() => onPick(conflict.name, path)}
									/>
									<span className="truncate">{path}</span>
								</label>
							))}
						</fieldset>
					</li>
				))}
			</ul>
		</section>
	) : null;
};

const isEmpty = (plan: GlobalsPlan) =>
	plan.moves.length +
		plan.links.length +
		plan.backups.length +
		(plan.joins?.length ?? 0) +
		(plan.conflicts?.length ?? 0) ===
	0;

const Body = ({ device, onDone }: { device: DeviceId; onDone: () => void }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const name = useView((s) => deviceName(s.view, device));
	const [ticked, setTicked] = useState(false);
	const [picks, setPicks] = useState<Record<string, string>>({});
	const { note, busy, run } = useSkillAction();
	const plan = useQuery({
		queryKey: ["skills-globals-plan", device],
		queryFn: async () =>
			readOf(await source.planGlobalSkills(device), "skills_globals_plan").plan,
		gcTime: 0,
		retry: false,
	});
	const apply = async (hash: string) => {
		const picked = Object.values(picks);
		if (
			await run(t("Tidying"), t("Tidied"), () =>
				source.applyGlobalSkills(device, hash, picked)
			)
		) {
			onDone();
		}
	};
	return (
		<div className="flex flex-col gap-4">
			<Title>{t("Tidy global skills on {{name}}", { name: name })}</Title>
			<p className="text-[12.5px] text-secondary-foreground">
				{t(
					"Your own global skills move into the library, so you pick them per project. The skills every agent gets stay global, as links MaxHQ keeps."
				)}
			</p>
			{plan.isPending ? (
				<p className="text-[12.5px] text-muted-foreground">
					{t("Reading the global folders")}
				</p>
			) : plan.isError ? (
				<p role="alert" className="text-[12.5px] text-status-error-text">
					{plan.error.message}
				</p>
			) : isEmpty(plan.data) ? (
				<p className="text-[12.5px] text-muted-foreground">
					{t("There's nothing to tidy on {{name}}.", { name: name })}
				</p>
			) : (
				<>
					<div className="flex max-h-[38vh] flex-col gap-3 overflow-y-auto">
						<List title={t("Move into the library")} paths={plan.data.moves} />
						<Joins joins={plan.data.joins ?? []} />
						<Conflicts
							conflicts={plan.data.conflicts ?? []}
							picks={picks}
							onPick={(name, path) => setPicks((now) => ({ ...now, [name]: path }))}
						/>
						<List title={t("Links that appear")} paths={plan.data.links} />
						<List title={t("Copies that go to the backup")} paths={plan.data.backups} />
					</div>
					<ul className="flex list-disc flex-col gap-1 pl-4 text-xs text-muted-foreground">
						<li>
							{t(
								"Nothing is deleted. Moved folders go to a dated backup folder, and Undo puts them back."
							)}
						</li>
						<li>
							{t(
								"MaxHQ writes only skill folders and links. It never touches settings.json, settings.local.json, config.toml or skillOverrides."
							)}
						</li>
						<li>
							{t(
								"Hiding a plugin or system skill stays your job, in that CLI's own settings."
							)}
						</li>
					</ul>
					<label className="flex items-center gap-2 text-[12.5px] text-foreground">
						<Checkbox
							aria-label={t("Tidy {{name}}", { name: name })}
							checked={ticked}
							onCheckedChange={setTicked}
						/>
						{t("Tidy")}
						{name}
					</label>
				</>
			)}
			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} className="mr-auto" />
				<Dialog.Close className={SECONDARY}>{t("Cancel")}</Dialog.Close>
				{plan.data && !isEmpty(plan.data) ? (
					<button
						type="button"
						className={PRIMARY}
						disabled={!ticked || busy}
						onClick={() => void apply(plan.data.hash)}
					>
						{t("Apply")}
					</button>
				) : null}
			</div>
		</div>
	);
};

/** The tidy dialog for one device. `onDone` runs once the device tidied. */
export const TidyDialog = ({
	device,
	onOpenChange,
	onDone,
}: {
	/** The device to tidy, or null while the dialog is shut. */
	device: DeviceId | null;
	onOpenChange: (open: boolean) => void;
	onDone: () => void;
}) => {
	useI18n();
	return (
		<Dialog.Root open={device !== null} onOpenChange={onOpenChange}>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={cn(POPUP, "w-[min(600px,calc(100vw-32px))]")}>
					{device ? <Body key={device} device={device} onDone={onDone} /> : null}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
