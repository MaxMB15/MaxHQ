// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The skills library page in the settings: the device that holds it, each
// source with its trust, where it comes from and its skills with their
// badges, a form for a new source, the skills every agent gets, and for
// each device a tidy of its global skills with an Undo.

import { useI18n } from "@maxhq/shared/i18n";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import {
	ConfirmButton,
	FIELD,
	Label,
	PRIMARY,
	Row,
	SECONDARY,
	Section,
	StatusNote,
} from "@/settings/controls";
import { Badge } from "@/tasks/activity";
import type { DeviceId, SkillLibrary, SkillSource } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { skillBadges, skillLibrary, sourceWhere } from "@maxhq/shared/skills/library";
import { devicesWithSkills } from "@maxhq/shared/skills/project";
import { EMPTY_SOURCE, planSource, type SourceDraft } from "@maxhq/shared/skills/source-form";
import { readOf } from "@maxhq/shared/pulls/read";
import { formatAgo } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import { SkillBadges, useSkillAction } from "./look";
import { TidyDialog } from "./tidy-dialog";

const SourceBlock = ({ library, source }: { library: SkillLibrary; source: SkillSource }) => {
	const { t } = useI18n();

	const { source: data } = useServices();
	const now = useNow(60_000);
	const { note, busy, run } = useSkillAction();
	const where = sourceWhere(source.kind);
	const parts = [
		source.default_ref
			? t("{{where}} at {{default_ref}}", { where: where, default_ref: source.default_ref })
			: where,
		source.commit,
		source.fetched_at
			? t("fetched {{value1}}", { value1: formatAgo(source.fetched_at, now) })
			: t("not fetched yet"),
		source.unpushed ? t("not pushed") : null,
	].filter(Boolean);
	return (
		<div
			role="group"
			aria-label={source.name}
			className="border-t border-sidebar-border first:border-t-0"
		>
			<div className="flex items-center gap-4 px-4 py-3">
				<div className="flex min-w-0 flex-1 flex-col gap-0.5">
					<span className="flex items-center gap-2 text-[12.5px] text-foreground">
						<span className="truncate font-[600]">{source.name}</span>
						<Badge tone={source.trust === "own" ? "good" : "plain"}>
							{source.trust === "own" ? t("Yours") : t("Third party")}
						</Badge>
					</span>
					<span className="truncate font-mono text-xs text-muted-foreground">
						{parts.join(" · ")}
					</span>
					{source.problem ? (
						<span className="text-xs text-status-error-text">{source.problem}</span>
					) : null}
					<StatusNote note={note} />
				</div>
				<div className="flex shrink-0 items-center gap-2.5">
					{source.unpushed ? (
						<button
							type="button"
							className={SECONDARY}
							disabled={busy}
							onClick={() =>
								void run(t("Pushing"), t("Pushed"), () =>
									data.pushSkillSource(library.device, source.id)
								)
							}
						>
							{t("Push")}
						</button>
					) : null}
					<button
						type="button"
						className={SECONDARY}
						disabled={busy}
						onClick={() =>
							void run(t("Fetching"), t("Fetched"), () =>
								data.fetchSkillSource(library.device, source.id)
							)
						}
					>
						{t("Fetch")}
					</button>
					<ConfirmButton
						label={t("Remove")}
						question={t("Remove {{name}}?", { name: source.name })}
						confirm={t("Remove")}
						disabled={busy}
						onConfirm={() =>
							void run(t("Removing"), t("Removed"), () =>
								data.removeSkillSource(library.device, source.id)
							)
						}
					/>
				</div>
			</div>
			{source.skills.length === 0 ? (
				<p className="px-4 pb-3 text-xs text-muted-foreground">
					{t("No skills found in it yet.")}
				</p>
			) : (
				<ul
					aria-label={t("Skills in {{name}}", { name: source.name })}
					className="flex flex-col pb-2"
				>
					{source.skills.map((skill) => (
						<li key={skill.name} className="flex items-start gap-3 px-4 py-1.5">
							<div className="flex min-w-0 flex-1 flex-col">
								<span className="font-mono text-[12px] text-foreground">
									{skill.name}
								</span>
								<span className="truncate text-xs text-muted-foreground">
									{skill.description}
								</span>
							</div>
							<SkillBadges badges={skillBadges(skill, source)} />
						</li>
					))}
				</ul>
			)}
		</div>
	);
};

const AddSource = ({ library }: { library: SkillLibrary }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [draft, setDraft] = useState<SourceDraft>(EMPTY_SOURCE);
	const { note, busy, run, setNote } = useSkillAction();
	const plan = planSource(library, draft);
	const set = (change: Partial<SourceDraft>) => {
		setNote(null);
		setDraft({ ...draft, ...change });
	};
	const add = async () => {
		if (!plan.ok) return;
		const added = await run(t("Adding"), t("Added"), () =>
			source.addSkillSource(library.device, plan.name, plan.origin, plan.ref, plan.trust)
		);
		if (added) setDraft(EMPTY_SOURCE);
	};
	return (
		<form
			aria-label={t("Add a source")}
			className="flex flex-col gap-3 border-t border-sidebar-border px-4 py-3"
			onSubmit={(e) => {
				e.preventDefault();
				void add();
			}}
		>
			<label className="flex flex-col gap-1">
				<Label>{t("Repo or folder")}</Label>
				<input
					className={FIELD}
					value={draft.where}
					placeholder={t("owner/repo, a git URL, or /path/to/skills")}
					onChange={(e) => set({ where: e.target.value })}
				/>
			</label>
			<div className="flex gap-3">
				<label className="flex flex-1 flex-col gap-1">
					<Label>{t("Name")}</Label>
					<input
						className={FIELD}
						value={draft.name}
						placeholder={t("From the repo")}
						onChange={(e) => set({ name: e.target.value })}
					/>
				</label>
				<label className="flex flex-1 flex-col gap-1">
					<Label>{t("Branch or tag")}</Label>
					<input
						className={FIELD}
						value={draft.ref}
						placeholder={t("Its default branch")}
						onChange={(e) => set({ ref: e.target.value })}
					/>
				</label>
			</div>
			<div className="flex items-center gap-2.5">
				<Switch
					aria-label={t("A third party's skills")}
					checked={draft.thirdParty}
					onCheckedChange={(thirdParty) => set({ thirdParty })}
				/>
				<span className="text-xs text-secondary-foreground">
					{t("A third party's skills, pinned until you update them")}
				</span>
			</div>
			<div className="flex items-center justify-end gap-2.5">
				{!plan.ok && draft.where.trim() ? (
					<span className="mr-auto text-[11.5px] text-muted-foreground">
						{plan.reason}
					</span>
				) : (
					<StatusNote note={note} className="mr-auto" />
				)}
				<button type="submit" className={PRIMARY} disabled={!plan.ok || busy}>
					{t("Add source")}
				</button>
			</div>
		</form>
	);
};

/** The key of a device's tidy plan, which also says whether it has a tidy to undo. */
const planKey = (device: DeviceId) => ["skills-globals-plan", device];

const DeviceTidy = ({ device, onTidy }: { device: DeviceId; onTidy: () => void }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const queries = useQueryClient();
	const name = useView((s) => deviceName(s.view, device));
	const { note, busy, run } = useSkillAction();
	const plan = useQuery({
		queryKey: planKey(device),
		queryFn: async () =>
			readOf(await source.planGlobalSkills(device), "skills_globals_plan").plan,
		retry: false,
	});
	const undo = async () => {
		if (await run(t("Putting back"), t("Put back"), () => source.undoGlobalSkills(device))) {
			await queries.invalidateQueries({ queryKey: planKey(device) });
		}
	};
	return (
		<Row title={name} sub={note ? <StatusNote note={note} /> : undefined}>
			{plan.data?.undo ? (
				<button
					type="button"
					className={SECONDARY}
					disabled={busy}
					onClick={() => void undo()}
				>
					{t("Undo")}
				</button>
			) : null}
			<button type="button" className={SECONDARY} onClick={onTidy}>
				{t("Tidy global skills")}
			</button>
		</Row>
	);
};

/** The page's headings and labels, for the settings search. */
export const SKILLS_LIBRARY_TERMS: readonly string[] = [
	"Sources",
	"Add a source",
	"Repo or folder",
	"Branch or tag",
	"A third party's skills",
	"Every agent",
	"Skills every agent gets",
	"Global skills",
];

export const SkillsSettingsPage = () => {
	const { t } = useI18n();

	const queries = useQueryClient();
	const view = useView((s) => s.view);
	const library = skillLibrary(view);
	const devices = devicesWithSkills(view);
	const [tidying, setTidying] = useState<DeviceId | null>(null);
	const [tidied, setTidied] = useState<string | null>(null);
	if (!library) {
		return (
			<p className="text-[12.5px] text-muted-foreground">
				{t("No device holds a skill library yet. It comes with a newer maxd.")}
			</p>
		);
	}
	return (
		<>
			<Section
				title={t("Sources")}
				hint={t("Held by {{value1}}", { value1: deviceName(view, library.device) })}
			>
				{library.sources.length === 0 ? (
					<p className="px-4 py-3 text-xs text-muted-foreground">
						{t("No sources yet. Add a repo or folder of skills below.")}
					</p>
				) : (
					library.sources.map((source) => (
						<SourceBlock key={source.id} library={library} source={source} />
					))
				)}
				<AddSource library={library} />
			</Section>
			<Section
				title={t("Every agent")}
				hint={t("Global on each device, so projects can't take them")}
			>
				{library.always.length === 0 ? (
					<p className="px-4 py-3 text-xs text-muted-foreground">{t("None yet.")}</p>
				) : (
					<ul
						aria-label={t("Skills every agent gets")}
						className="flex flex-wrap gap-1.5 px-4 py-3"
					>
						{library.always.map((name) => (
							<li
								key={name}
								className="rounded-[6px] border border-track bg-secondary px-2 py-0.5 font-mono text-[12px] text-foreground"
							>
								{name}
							</li>
						))}
					</ul>
				)}
			</Section>
			<Section
				title={t("Global skills")}
				hint={t("Move your own into the library, device by device")}
			>
				{devices.map((device) => (
					<DeviceTidy key={device} device={device} onTidy={() => setTidying(device)} />
				))}
				{tidied ? (
					<p role="status" className="px-4 pb-3 text-[11.5px] text-muted-foreground">
						{tidied}
					</p>
				) : null}
			</Section>
			<TidyDialog
				device={tidying}
				onOpenChange={(open) => {
					if (!open) setTidying(null);
				}}
				onDone={() => {
					setTidied(
						t("Tidied {{value1}}. Undo puts it back.", {
							value1: deviceName(view, tidying!),
						})
					);
					void queries.invalidateQueries({ queryKey: planKey(tidying!) });
					setTidying(null);
				}}
			/>
		</>
	);
};
