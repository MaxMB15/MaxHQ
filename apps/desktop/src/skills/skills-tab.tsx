// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's Skills tab: a banner for checkouts whose copies changed,
// the proposals waiting, then on each device that keeps the project's
// skills the library to pick from, the project's own set with Update, Pin
// and Apply and its devflow pack, and what each CLI sees there as the
// device's scan found it.
// Every change to the set goes to the device at once; Apply puts the set
// into the checkouts.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { FIELD, PRIMARY, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { Badge } from "@/tasks/activity";
import type {
	AgentPlace,
	DeviceId,
	ProjectId,
	SkillLibrary,
	SkillLock,
	SkillProposalId,
} from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	cliLabel,
	type CliSkills,
	markLabel,
	originLabel,
	skillsByCli,
} from "@maxhq/shared/skills/effective";
import {
	addFromProject,
	cantLock,
	driftOf,
	outsideOf,
	lockedFrom,
	searchLibrary,
	skillBadges,
	skillLibrary,
	skillProposals,
} from "@maxhq/shared/skills/library";
import {
	budgetsOf,
	EMPTY_LOCK,
	hasUpdates,
	hideHint,
	lockedWhere,
	needsApply,
	otherLocks,
	pinSkill,
	projectSkillsOn,
	proposalFrom,
	pushedOut,
	scanPlaceOf,
	skillDevices,
	updatedLock,
	updateOf,
	withoutSkill,
} from "@maxhq/shared/skills/project";
import { rescanSkills, useSkillScan, useSteadyPlace } from "@maxhq/shared/skills/scan";
import { formatAgo, listInWords } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import { BudgetMeter, SkillBadges, useSkillAction } from "./look";
import { PackRow } from "./pack-row";
import { ProposalViewer } from "./proposal-viewer";

const SMALL = cn(SECONDARY, "h-[26px] px-2 text-xs");

const OutsideBanner = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const found = outsideOf(view, project);
	if (found.length === 0) return null;
	return (
		<div
			role="region"
			aria-label={t("Skills that can't land")}
			className="flex flex-col gap-2 rounded-[11px] border border-status-busy/45 bg-status-busy/10 px-4 py-3"
		>
			{found.map(({ device, checkout }) => (
				<p
					key={`${device}:${checkout.path}`}
					className="selectable min-w-0 text-[12.5px] text-foreground"
				>
					{t(
						"Skills can't land through {{value1}} in {{path}} on {{value3}}: a link there leads out of the checkout.",
						{
							value1: listInWords(checkout.outside ?? []),
							path: checkout.path,
							value3: deviceName(view, device),
						}
					)}
				</p>
			))}
		</div>
	);
};

const DriftBanner = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const { note, busy, run } = useSkillAction();
	const drift = driftOf(view, project);
	if (drift.length === 0) return null;
	return (
		<div
			role="region"
			aria-label={t("Changed skills")}
			className="flex flex-col gap-2 rounded-[11px] border border-status-busy/45 bg-status-busy/10 px-4 py-3"
		>
			{drift.map(({ device, checkout }) => (
				<div key={`${device}:${checkout.path}`} className="flex items-center gap-3">
					<p className="selectable min-w-0 flex-1 text-[12.5px] text-foreground">
						{t("{{value1}} changed in {{path}} on {{value3}} since it landed.", {
							value1: listInWords(checkout.drift),
							path: checkout.path,
							value3: deviceName(view, device),
						})}
					</p>
					<button
						type="button"
						className={SECONDARY}
						disabled={busy}
						onClick={() =>
							void run(t("Restoring"), t("Restored"), () =>
								source.applySkills(device, project)
							)
						}
					>
						{t("Restore")}
					</button>
				</div>
			))}
			<StatusNote note={note} />
		</div>
	);
};

const Proposals = ({
	project,
	onOpen,
}: {
	project: ProjectId;
	onOpen: (id: SkillProposalId) => void;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const proposals = skillProposals(view, project);
	if (proposals.length === 0) return null;
	return (
		<Section title={t("Proposals")} hint={t("Changes agents want in your skills")}>
			{proposals.map((proposal) => (
				<div
					key={proposal.id}
					className="flex items-center gap-3 border-t border-sidebar-border px-4 py-2.5 first:border-t-0"
				>
					<div className="flex min-w-0 flex-1 flex-col">
						<span className="selectable font-mono text-[12px] text-foreground">
							{proposal.name}
						</span>
						<span
							title={t("From {{value1}}, {{value2}}", {
								value1: proposalFrom(view, proposal),
								value2: formatAgo(proposal.created_at, now),
							})}
							className="selectable truncate text-xs text-muted-foreground"
						>
							{t("From {{value1}}, {{value2}}", {
								value1: proposalFrom(view, proposal),
								value2: formatAgo(proposal.created_at, now),
							})}
						</span>
					</div>
					<button type="button" className={SECONDARY} onClick={() => onOpen(proposal.id)}>
						{t("Review")}
					</button>
				</div>
			))}
		</Section>
	);
};

const LibraryPicker = ({
	library,
	lock,
	busy,
	onAdd,
}: {
	library: SkillLibrary;
	lock: SkillLock;
	busy: boolean;
	onAdd: (lock: SkillLock, name: string) => void;
}) => {
	const { t } = useI18n();

	const [query, setQuery] = useState("");
	const found = searchLibrary(library, query);
	return (
		<Section title={t("Library")} className="min-w-0">
			<div className="px-3 pt-3">
				<input
					aria-label={t("Search the library")}
					className={FIELD}
					placeholder={t("Search skills")}
					value={query}
					onChange={(e) => setQuery(e.target.value)}
				/>
			</div>
			<ul
				aria-label={t("Library skills")}
				className="flex max-h-[360px] flex-col overflow-y-auto py-2"
			>
				{found.length === 0 ? (
					<li className="px-4 py-2 text-xs text-muted-foreground">
						{t("No skills match")}
					</li>
				) : null}
				{found.map((item) => {
					const why = cantLock(library, lock, item.skill.name);
					return (
						<li
							key={`${item.source.id}:${item.skill.name}`}
							className="flex items-start gap-2 px-3 py-1.5"
						>
							<div className="flex min-w-0 flex-1 flex-col gap-0.5">
								<span className="selectable font-mono text-[12px] text-foreground">
									{item.skill.name}
								</span>
								<span
									title={why ?? item.skill.description}
									className="selectable truncate text-xs text-muted-foreground"
								>
									{why ?? item.skill.description}
								</span>
								<SkillBadges badges={skillBadges(item.skill, item.source)} />
							</div>
							<button
								type="button"
								aria-label={t("Add {{name}}", { name: item.skill.name })}
								className={SMALL}
								disabled={busy || why !== null}
								onClick={() =>
									onAdd(
										{ ...lock, skills: [...lock.skills, lockedFrom(item)] },
										item.skill.name
									)
								}
							>
								{t("Add")}
							</button>
						</li>
					);
				})}
			</ul>
		</Section>
	);
};

const CliGroup = ({
	group,
	budget,
	mac,
}: {
	group: CliSkills;
	budget: ReturnType<typeof budgetsOf>[number] | undefined;
	mac: boolean;
}) => {
	const { t } = useI18n();

	const hint = hideHint(group.cli);
	const fixed = group.skills.some((s) => s.place === "plugin" || s.place === "system");
	return (
		<div
			role="group"
			aria-label={cliLabel(group.cli)}
			className="border-t border-sidebar-border first:border-t-0"
		>
			<div className="flex items-center justify-between gap-3 px-4 pt-3 pb-1">
				<h3 className="text-[12.5px] font-[600] text-foreground">{cliLabel(group.cli)}</h3>
				{budget ? <BudgetMeter budget={budget} /> : null}
			</div>
			<ul className="flex flex-col pb-2">
				{group.skills.map((skill) => (
					<li
						key={skill.path}
						title={skill.path}
						className="flex items-start gap-2 px-4 py-1"
					>
						<div className="flex min-w-0 flex-1 flex-col">
							<span className="selectable font-mono text-[12px] text-foreground">
								{skill.name}
							</span>
							{skill.mark ? (
								<span
									className={cn(
										"text-xs",
										skill.mark === "conflict" || skill.mark === "shadowed"
											? "text-status-busy-text"
											: "text-muted-foreground"
									)}
								>
									{markLabel(skill.mark)}
								</span>
							) : null}
						</div>
						<Badge tone="plain">{originLabel(skill)}</Badge>
					</li>
				))}
			</ul>
			{fixed && hint ? (
				<p className="selectable px-4 pb-3 text-xs text-muted-foreground">{hint}</p>
			) : null}
			{group.cli === "cursor" && mac ? (
				<p className="selectable px-4 pb-3 text-xs text-muted-foreground">
					{t(
						"Cursor's own skills in ~/.cursor on this Mac aren't read, so they don't show here."
					)}
				</p>
			) : null}
		</div>
	);
};

const Effective = ({
	project,
	device,
	place,
}: {
	project: ProjectId;
	device: DeviceId;
	place: AgentPlace | null;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const scan = useSkillScan(source, place ? device : null, place);
	const mac = view.tables.device.get(device)?.os === "macos";
	const skills = scan?.skills ?? null;
	const budgets = skills ? budgetsOf(view, project, skills) : [];
	return (
		<Section
			title={t("What each CLI sees")}
			label={t("What each CLI sees")}
			hint={
				place ? (
					<button
						type="button"
						className={SMALL}
						disabled={scan?.state === "scanning"}
						onClick={() => void rescanSkills(source, device, place)}
					>
						{t("Rescan")}
					</button>
				) : undefined
			}
		>
			{!place ? (
				<p className="selectable px-4 py-3 text-xs text-muted-foreground">
					{t("There's no checkout of this project on {{value1}} to look in.", {
						value1: deviceName(view, device),
					})}
				</p>
			) : skills === null ? (
				<p
					role={scan?.state === "failed" ? "alert" : undefined}
					className={cn(
						"selectable px-4 py-3 text-xs",
						scan?.state === "failed"
							? "text-status-error-text"
							: "text-muted-foreground"
					)}
				>
					{scan?.state === "failed" ? scan.error : t("Looking")}
				</p>
			) : (
				skillsByCli(skills).map((group) => (
					<CliGroup
						key={group.cli}
						group={group}
						budget={budgets.find((b) => b.cli === group.cli)}
						mac={mac}
					/>
				))
			)}
		</Section>
	);
};

const OnDevice = ({
	project,
	device,
	library,
}: {
	project: ProjectId;
	device: DeviceId;
	library: SkillLibrary;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const { note, busy, run } = useSkillAction();
	const skills = projectSkillsOn(view, project, device);
	const lock = skills?.lock ?? EMPTY_LOCK;
	const place = useSteadyPlace(scanPlaceOf(view, project, device));
	const scan = useSkillScan(source, place ? device : null, place);
	const pushed = scan?.skills ? pushedOut(view, project, library, lock, scan.skills) : [];
	const others = otherLocks(view, project, device);

	const save = (next: SkillLock, doing: string, done: string) =>
		void run(doing, done, () => source.setProjectSkills(device, project, next));
	const apply = async () => {
		const applied = await run(t("Applying"), t("Applied"), () =>
			source.applySkills(device, project)
		);
		if (applied && place) void rescanSkills(source, device, place);
	};

	return (
		<>
			<div className="grid grid-cols-2 items-start gap-3">
				<LibraryPicker
					library={library}
					lock={lock}
					busy={busy}
					onAdd={(next, name) =>
						save(next, t("Adding"), t("Added {{name}}", { name: name }))
					}
				/>
				<Section title={t("This project")} className="min-w-0">
					{lock.skills.length === 0 ? (
						<p className="px-4 py-3 text-xs text-muted-foreground">
							{t("No skills yet. Add some from the library.")}
						</p>
					) : (
						<ul aria-label={t("Project skills")} className="flex flex-col py-2">
							{lock.skills.map((locked) => {
								const update = updateOf(library, locked);
								return (
									<li
										key={locked.name}
										className="flex flex-col gap-1 px-3 py-1.5"
									>
										<span className="flex items-center gap-2">
											<span
												title={locked.name}
												className="selectable min-w-0 flex-1 truncate font-mono text-[12px] text-foreground"
											>
												{locked.name}
											</span>
											{locked.pinned ? (
												<Badge tone="info">{t("Pinned")}</Badge>
											) : null}
										</span>
										<span
											title={lockedWhere(library, locked)}
											className="selectable truncate text-xs text-muted-foreground"
										>
											{lockedWhere(library, locked)}
										</span>
										<span className="flex gap-1.5">
											{update ? (
												<button
													type="button"
													aria-label={t("Update {{name}}", {
														name: locked.name,
													})}
													className={SMALL}
													disabled={busy}
													onClick={() =>
														save(
															{
																...lock,
																skills: lock.skills.map((s) =>
																	s.name === locked.name
																		? update
																		: s
																),
															},
															t("Updating"),
															t("Updated {{name}}", {
																name: locked.name,
															})
														)
													}
												>
													{t("Update")}
												</button>
											) : null}
											<button
												type="button"
												aria-label={`${locked.pinned ? t("Unpin") : t("Pin")} ${locked.name}`}
												className={SMALL}
												disabled={busy}
												onClick={() =>
													save(
														pinSkill(lock, locked.name, !locked.pinned),
														t("Saving"),
														t("Saved")
													)
												}
											>
												{locked.pinned ? t("Unpin") : t("Pin")}
											</button>
											<button
												type="button"
												aria-label={t("Remove {{name}}", {
													name: locked.name,
												})}
												className={SMALL}
												disabled={busy}
												onClick={() =>
													save(
														withoutSkill(lock, locked.name),
														t("Removing"),
														t("Removed {{name}}", { name: locked.name })
													)
												}
											>
												{t("Remove")}
											</button>
										</span>
									</li>
								);
							})}
						</ul>
					)}
					<PackRow
						library={library}
						lock={lock}
						checkouts={skills?.checkouts ?? []}
						busy={busy}
						onSave={save}
						onRetry={() => void apply()}
					/>
					<div className="flex flex-col gap-3 border-t border-sidebar-border px-3 py-3">
						<div className="flex flex-wrap gap-2">
							{hasUpdates(library, lock) ? (
								<button
									type="button"
									className={SECONDARY}
									disabled={busy}
									onClick={() =>
										save(
											updatedLock(library, lock),
											t("Updating"),
											t("Updated")
										)
									}
								>
									{t("Update all")}
								</button>
							) : null}
							{others.length > 0 ? (
								<select
									aria-label={t("Add from project")}
									className={cn(FIELD, "w-auto")}
									value=""
									disabled={busy}
									onChange={(e) => {
										const from = others.find(
											(p) => p.project === e.target.value
										);
										if (from) {
											save(
												addFromProject(library, lock, from.lock),
												t("Adding"),
												t("Added")
											);
										}
									}}
								>
									<option value="">{t("Add from project")}</option>
									{others.map((p) => (
										<option key={p.project} value={p.project}>
											{view.tables.project.get(p.project)?.name ?? p.project}
										</option>
									))}
								</select>
							) : null}
						</div>
						<label className="flex items-center gap-2.5">
							<Switch
								aria-label={t("Commit the skills")}
								checked={lock.committed}
								disabled={busy}
								onCheckedChange={(committed) =>
									save({ ...lock, committed }, t("Saving"), t("Saved"))
								}
							/>
							<span className="text-xs text-secondary-foreground">
								{t("Commit the skills, so everyone who clones the repo gets them")}
							</span>
						</label>
						{pushed.length > 0 ? (
							<p role="alert" className="text-xs text-status-busy-text">
								{t(
									"Applying pushes {{value1}} past its skill listing, so it drops descriptions.",
									{ value1: listInWords(pushed.map(cliLabel)) }
								)}
							</p>
						) : null}
						<div className="flex items-center gap-2.5">
							<StatusNote note={note} className="mr-auto" />
							{needsApply(skills) ? (
								<span className="text-xs text-muted-foreground">
									{t("Not in the checkouts yet")}
								</span>
							) : null}
							<button
								type="button"
								className={PRIMARY}
								disabled={busy || !skills}
								onClick={() => void apply()}
							>
								{t("Apply")}
							</button>
						</div>
					</div>
				</Section>
			</div>
			<Effective project={project} device={device} place={place} />
		</>
	);
};

/** The tab's headings and labels, for the settings search. */
export const PROJECT_SKILLS_TERMS: readonly string[] = [
	"Skills",
	"Proposals",
	"Library",
	"What each CLI sees",
	"This project",
	"Devflow pack",
	"Add from project",
	"Commit the skills",
];

/** A project's Skills tab. */
export const ProjectSkillsTab = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const devices = skillDevices(view, project);
	const library = skillLibrary(view);
	const [picked, setPicked] = useState<DeviceId | null>(null);
	const [viewing, setViewing] = useState<SkillProposalId | null>(null);
	// It starts on the device that holds the library, your own, when it keeps the project.
	const start = library && devices.includes(library.device) ? library.device : devices[0];
	const device = picked && devices.includes(picked) ? picked : start;
	const proposal = skillProposals(view, project).find((p) => p.id === viewing) ?? null;
	return (
		<div className="flex flex-col gap-[18px]">
			<OutsideBanner project={project} />
			<DriftBanner project={project} />
			<Proposals project={project} onOpen={setViewing} />
			{!device ? (
				<p className="text-xs text-muted-foreground">
					{t(
						"A device keeps a project's skills once it hosts a workspace of it and runs a newer maxd."
					)}
				</p>
			) : (
				<>
					{devices.length > 1 ? (
						<div role="group" aria-label={t("Device")} className="flex gap-1">
							{devices.map((d) => (
								<button
									key={d}
									type="button"
									aria-pressed={d === device}
									onClick={() => setPicked(d)}
									className="h-[28px] rounded-[6px] px-2.5 text-[12.5px] text-secondary-foreground hover:bg-secondary aria-pressed:bg-selected aria-pressed:text-foreground"
								>
									{deviceName(view, d)}
								</button>
							))}
						</div>
					) : null}
					{library ? (
						<OnDevice
							key={device}
							project={project}
							device={device}
							library={library}
						/>
					) : (
						<p className="text-xs text-muted-foreground">
							{t("No device holds a skill library yet.")}
						</p>
					)}
				</>
			)}
			<ProposalViewer
				proposal={proposal}
				onOpenChange={(open) => {
					if (!open) setViewing(null);
				}}
				onDone={() => setViewing(null)}
			/>
		</div>
	);
};
