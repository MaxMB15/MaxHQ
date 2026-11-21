// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's page in the settings, on two tabs. General has its
// workspaces, then what each device keeps for it: each device keeps its
// own copy, so each gets its own form and its own Save, and its own
// "Always allow" command rules. Skills has the project's skills.

import { Tabs } from "@base-ui/react/tabs";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { CopyIcon } from "@/components/copy-field";
import { Switch } from "@/components/ui/switch";
import type { CommandRule, DeviceId, ProjectId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { settleDaysText } from "@maxhq/shared/settings/inbox";
import { deviceName } from "@maxhq/shared/view/select";
import { isLive } from "@maxhq/shared/workspaces/state";
import { ProjectSkillsTab } from "@/skills/skills-tab";
import { ProjectWorkspaces } from "@/workspaces/workspace-list";

import { FIELD, Label, PRIMARY, Row, SECONDARY, Section, StatusNote } from "./controls";
import {
	allowedWorkflowEnvText,
	commandRulesOn,
	deviceSettleDays,
	draftOf,
	firstSettingsOn,
	planProjectSettings,
	projectCapabilities,
	type ProjectDraft,
	projectDevices,
	type ProjectEdit,
	projectSettingsOn,
	ruleKey,
	sameDraft,
	shownDraft,
} from "./project";
import { failureOf, type Note } from "./result";
import type { ProjectTab } from "./store";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/**
 * The commands you answered "Always allow" for, which the device answers
 * for you from then on. Each one can go.
 */
const CommandRules = ({
	project,
	device,
	live,
}: {
	project: ProjectId;
	device: DeviceId;
	live: boolean;
}) => {
	useI18n();
	const { source } = useServices();
	const rules = useView((s) => commandRulesOn(s.view, project, device));
	const [note, setNote] = useState<Note | null>(null);
	const [busy, setBusy] = useState(false);

	const remove = async (rule: CommandRule) => {
		if (busy) return;
		setBusy(true);
		setNote(null);
		try {
			const failure = failureOf(await source.removeCommandRule(device, project, rule));
			if (failure) setNote({ text: failure, error: true });
		} catch (error) {
			console.warn(translate("Couldn't remove the command rule"), error);
			setNote({ text: translate("Couldn't remove it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	return (
		<div className="flex flex-col gap-2 border-t border-sidebar-border px-4 py-3">
			<div className="flex flex-col gap-0.5">
				<span className="text-[12.5px] text-foreground">
					{translate("Allowed commands")}
				</span>
				<span className="text-xs text-muted-foreground">
					{translate(
						'Commands you answered "Always allow" for. Agents here run them without asking, except in Plan. A rule keeps a command and its subcommand, so "git show" covers any git show. One marked exact covers only that line.'
					)}
				</span>
			</div>
			{rules.length === 0 ? (
				<span className="text-xs text-muted-foreground">{translate("None yet.")}</span>
			) : (
				<ul aria-label={translate("Allowed commands")} className="flex flex-col gap-1.5">
					{rules.map((rule) => (
						<li key={ruleKey(rule)} className="flex items-center gap-2">
							<span className="flex min-w-0 flex-1 items-center gap-0.5">
								<code
									title={rule.command}
									className="selectable min-w-0 truncate font-mono text-xs text-foreground"
								>
									{rule.command}
								</code>
								<CopyIcon
									text={rule.command}
									label={translate("Copy command")}
									className="size-[18px]"
								/>
							</span>
							{rule.exact && (
								<span className="text-[11px] text-muted-foreground">
									{translate("Exact")}
								</span>
							)}
							<button
								type="button"
								aria-label={translate("Remove {{command}}", {
									command: rule.command,
								})}
								className={cn(SECONDARY, "h-[26px] px-2.5 text-xs")}
								disabled={!live || busy}
								onClick={() => void remove(rule)}
							>
								{translate("Remove")}
							</button>
						</li>
					))}
				</ul>
			)}
			<StatusNote note={note} />
		</div>
	);
};

const OnDevice = ({ project, device }: { project: ProjectId; device: DeviceId }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const name = deviceName(view, device);
	const projectName = view.tables.project.get(project)?.name ?? project;
	const live = isLive(view, device, now);
	const identity = useQuery({ queryKey: ["git-identity"], queryFn: () => source.gitIdentity() });
	const kept = projectSettingsOn(view, project, device);
	const current = draftOf(kept);
	// The variables you allowed for every agent here, which turning this off forgets.
	const allowed = allowedWorkflowEnvText(kept, now);
	// A device that keeps nothing for the project yet starts the author as
	// this Mac's git identity, which a save then sends. Making a workspace or
	// starting an agent there sends it too.
	const first = firstSettingsOn(view, project, device, identity.data);
	const suggested = first?.git_author ?? null;
	const start = first ? draftOf(first) : current;
	// What you typed, kept until the device reports something new.
	const [edit, setEdit] = useState<ProjectEdit | null>(null);
	const [note, setNote] = useState<Note | null>(null);
	const [busy, setBusy] = useState(false);
	const draft = shownDraft(edit, current, start);
	const fromGit =
		suggested !== null && draft.name === suggested.name && draft.email === suggested.email;
	const set = (change: Partial<ProjectDraft>) => {
		setNote(null);
		setEdit({ draft: { ...draft, ...change }, from: current });
	};
	const plan = planProjectSettings(project, device, draft);
	const deviceDays = deviceSettleDays(view, device);
	// What this device's maxd is new enough to keep. One that isn't has no use for the field.
	const { settle: inbox, pulls, workflows, memory: caps } = projectCapabilities(view, device);
	const changed = !sameDraft(draft, current);

	const save = async () => {
		if (!plan.ok || !changed || busy) return;
		setBusy(true);
		setNote({ text: translate("Saving"), error: false });
		try {
			const failure = failureOf(await source.setProjectSettings(plan.settings));
			setNote(
				failure
					? { text: failure, error: true }
					: { text: translate("Saved"), error: false }
			);
		} catch (error) {
			console.warn(translate("Couldn't save the project settings"), error);
			setNote({ text: translate("Couldn't save it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	return (
		<Section
			title={translate("On {{name}}", { name: name })}
			label={translate("{{projectName}} on {{name}}", {
				projectName: projectName,
				name: name,
			})}
			hint={
				live
					? translate("Kept on the device")
					: translate("Connect to {{name}} to change it", { name: name })
			}
		>
			<form
				onSubmit={(e) => {
					e.preventDefault();
					void save();
				}}
			>
				<Row
					title={translate("Resume after a restart")}
					sub={translate(
						"When the device or a container restarts, agents that were busy carry on with a message naming what got cut off. Off leaves them paused."
					)}
				>
					<Switch
						aria-label={translate("Resume after a restart")}
						checked={draft.resume}
						disabled={!live}
						onCheckedChange={(on) => set({ resume: on })}
					/>
				</Row>
				<Row
					title={translate("Ports never offered")}
					sub={translate(
						"MaxHQ offers to open a port when something listens on it in a container. It never offers these, nor the ones it always skips, such as 22."
					)}
				>
					<input
						aria-label={translate("Ports never offered")}
						value={draft.ports}
						onChange={(e) => set({ ports: e.target.value })}
						placeholder={translate("Such as 9229, 35729")}
						spellCheck={false}
						autoComplete="off"
						disabled={!live}
						className={cn(FIELD, "w-[200px] font-mono text-xs")}
					/>
				</Row>
				<Row
					title={translate("Auto-settle after")}
					sub={
						inbox
							? translate(
									"Days with no activity before an agent in this project settles. Leave it empty to use {{name}}'s, which is {{value2}}. 0 never settles them.",
									{ name: name, value2: settleDaysText(deviceDays).toLowerCase() }
								)
							: translate("{{name}}'s maxd is too old for this. Update it first.", {
									name: name,
								})
					}
				>
					<span className="flex items-center gap-2 text-xs text-muted-foreground">
						<input
							aria-label={translate("Auto-settle after")}
							value={draft.autoSettle}
							onChange={(e) => set({ autoSettle: e.target.value })}
							placeholder={String(deviceDays)}
							inputMode="numeric"
							spellCheck={false}
							autoComplete="off"
							disabled={!live || !inbox}
							className={cn(FIELD, "w-[64px] text-right tabular-nums")}
						/>
						{translate("days")}
					</span>
				</Row>
				<Row
					title={translate("Settle an agent when its PR merges")}
					sub={
						pulls
							? translate(
									"Once a PR linked to an agent in this project merges, the agent settles. Off leaves it in the inbox."
								)
							: translate("{{name}}'s maxd is too old for this. Update it first.", {
									name: name,
								})
					}
				>
					<Switch
						aria-label={translate("Settle an agent when its PR merges")}
						checked={draft.settleOnMerge}
						disabled={!live || !pulls}
						onCheckedChange={(on) => set({ settleOnMerge: on })}
					/>
				</Row>
				<Row
					title={translate("Give agents the workflow's variables")}
					sub={
						workflows
							? translate(
									"Claude agents MaxHQ starts here can get the variables this project's workflow names, once you allow them on the card the agent shows. Off leaves only the agent's own name.{{value1}}",
									{ value1: allowed ? ` ${allowed}` : "" }
								)
							: translate("{{name}}'s maxd is too old for this. Update it first.", {
									name: name,
								})
					}
				>
					<Switch
						aria-label={translate("Give agents the workflow's variables")}
						checked={draft.workflowEnv}
						disabled={!live || !workflows}
						onCheckedChange={(on) => set({ workflowEnv: on })}
					/>
				</Row>
				<Row
					title={translate("Memory per container")}
					sub={
						caps
							? translate(
									"A soft limit for each container workspace of this project. Past it, {{name}} reclaims the container's memory first. Leave it empty to use the host's.",
									{ name: name }
								)
							: translate("{{name}}'s maxd is too old for this. Update it first.", {
									name: name,
								})
					}
				>
					<span className="flex items-center gap-2 text-xs text-muted-foreground">
						<input
							aria-label={translate("Memory per container")}
							value={draft.memory}
							onChange={(e) => set({ memory: e.target.value })}
							placeholder={translate("Host's")}
							inputMode="decimal"
							spellCheck={false}
							autoComplete="off"
							disabled={!live || !caps}
							className={cn(FIELD, "w-[64px] text-right tabular-nums")}
						/>
						GiB
					</span>
				</Row>
				<div className="flex flex-col gap-2.5 border-t border-sidebar-border px-4 py-3">
					<div className="flex flex-col gap-0.5">
						<span className="text-[12.5px] text-foreground">
							{translate("Commit author")}
						</span>
						<span className="text-xs text-muted-foreground">
							{translate(
								"The name and email on commits agents make in this project's containers. GitHub still shows the App as the one that pushed."
							)}
						</span>
						{fromGit && (
							<span className="text-xs text-muted-foreground">
								{translate("From this Mac's git config. Save to use it on ")}
								{name}.
							</span>
						)}
					</div>
					<div className="flex flex-wrap gap-2.5">
						<label className="flex min-w-[180px] flex-1 flex-col gap-1.5">
							<Label>{translate("Name")}</Label>
							<input
								value={draft.name}
								onChange={(e) => set({ name: e.target.value })}
								spellCheck={false}
								autoComplete="off"
								disabled={!live}
								className={FIELD}
							/>
						</label>
						<label className="flex min-w-[180px] flex-1 flex-col gap-1.5">
							<Label>{translate("Email")}</Label>
							<input
								type="email"
								value={draft.email}
								onChange={(e) => set({ email: e.target.value })}
								spellCheck={false}
								autoComplete="off"
								disabled={!live}
								className={FIELD}
							/>
						</label>
					</div>
				</div>
				<div className="flex items-center justify-end gap-3 border-t border-sidebar-border px-4 py-2.5">
					{!plan.ok ? (
						<span
							role="status"
							className="selectable text-[11.5px] text-status-error-text"
						>
							{plan.reason}
						</span>
					) : (
						<StatusNote note={note} />
					)}
					<button
						type="submit"
						className={PRIMARY}
						disabled={!live || !changed || !plan.ok || busy}
					>
						{translate("Save")}
					</button>
				</div>
			</form>
			<CommandRules project={project} device={device} live={live} />
		</Section>
	);
};

/** The General tab's headings and labels, for the settings search. */
export const PROJECT_TERMS: readonly string[] = [
	"General",
	"Workspaces",
	"Resume after a restart",
	"Ports never offered",
	"Auto-settle after",
	"Settle an agent when its PR merges",
	"Give agents the workflow's variables",
	"Memory per container",
	"Commit author",
	"Allowed commands",
];

/** One project's page in the settings. */
const TAB =
	"h-[28px] rounded-[6px] px-2.5 text-[12.5px] text-secondary-foreground hover:bg-secondary data-active:bg-selected data-active:text-foreground";

export const ProjectSettingsPage = ({
	project,
	tab = "general",
}: {
	project: ProjectId;
	tab?: ProjectTab;
}) => {
	useI18n();
	const view = useView((s) => s.view);
	const devices = projectDevices(view, project);
	return (
		<Tabs.Root defaultValue={tab} className="flex flex-col gap-[18px]">
			<Tabs.List className="flex gap-1">
				<Tabs.Tab value="general" className={TAB}>
					{translate("General")}
				</Tabs.Tab>
				<Tabs.Tab value="skills" className={TAB}>
					{translate("Skills")}
				</Tabs.Tab>
			</Tabs.List>
			<Tabs.Panel value="general" className="flex flex-col gap-[18px]">
				<ProjectWorkspaces project={project} />
				{devices.length === 0 ? (
					<p className="text-xs text-muted-foreground">
						{translate("A device keeps a project's settings once it hosts workspaces.")}
					</p>
				) : (
					devices.map((device) => (
						<OnDevice key={device} project={project} device={device} />
					))
				)}
			</Tabs.Panel>
			<Tabs.Panel value="skills">
				<ProjectSkillsTab project={project} />
			</Tabs.Panel>
		</Tabs.Root>
	);
};
