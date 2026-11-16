// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The projects side of the sidebar: the "Needs you" box, the pinned
// agents, then one group per project with its active agents, then the
// shelves. Idle agents over a day old fold into an "older" row, a
// project with PRs a GitHub App reaches gets a Pull requests entry first,
// one with notes gets a Notes entry, and one with workflow runs gets a
// Workflows entry. A project with notes or a live run and no agents shows
// for them. A project you
// hid stays out while its agents are quiet, and the Hidden projects entry
// under the groups lists it. The filter field above narrows all of it.
// The agent rows make one list, which Up and Down move through.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Menu } from "@base-ui/react/menu";
import { Ellipsis } from "lucide-react";
import { useMemo } from "react";

import { openAgent } from "@/app/open-agent";
import { useServices, useUi, useView } from "@/app/services";
import { keyHint, SHORTCUTS } from "@/layout/shortcuts";
import type { Timestamp } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { openNewAgent } from "@/new-agent/store";
import { ProjectWorkflows } from "@/workflow/containers";
import { searchWords } from "@maxhq/shared/lib/words";
import { foldKey } from "@maxhq/shared/ui/rows";
import { filterSidebar } from "@maxhq/shared/view/filter";
import { notesOf } from "@maxhq/shared/view/notes";
import type { ViewState } from "@maxhq/shared/view/replica";
import { projectRuns } from "@maxhq/shared/workflow/select";
import {
	hiddenProjectList,
	inboxSections,
	needsYou,
	type ProjectGroup,
	projectGroups,
} from "@maxhq/shared/view/select";

import { AgentRow, Chevron, useLookingAt } from "./agent-row";
import { pressAgent } from "./drag";
import { useFilter, useFilterFold } from "./filter";
import { HiddenProjects } from "./hidden-projects";
import { NotesEntry } from "./notes-entry";
import { usePrefs } from "./prefs";
import { PROJECT_MENU, ProjectContextMenu, ProjectMenuItems } from "./project-menu";
import { PullsEntry } from "./pulls-entry";
import { watchRowFocus } from "./row-focus";
import { PinnedList, Shelves } from "./shelves";

// The + and ⋯ at a project header's end, shown on hover and on keyboard focus.
const HEADER_ACTION =
	"grid size-5 shrink-0 place-items-center rounded-[5px] text-muted-foreground group-hover/header:opacity-100 hover:bg-accent hover:text-foreground focus-visible:opacity-100";

const Group = ({
	group,
	view,
	now,
	first,
}: {
	group: ProjectGroup;
	view: ViewState;
	now: Timestamp;
	first: boolean;
}) => {
	const { t } = useI18n();

	const lookingAt = useLookingAt();
	const stored = useUi((s) => s.collapsed.has(group.key));
	const showingOlder = useUi((s) => s.showingOlder.has(group.key));
	const toggleProject = useUi((s) => s.toggleProject);
	const [open, toggle] = useFilterFold(foldKey("project", group.key), !stored, () =>
		toggleProject(group.key)
	);
	const toggleOlder = useUi((s) => s.toggleOlder);
	const services = useServices();
	const { project } = group;
	// The project holding the agent you're looking at keeps its buttons showing.
	const selected = useUi((s) => s.selectedAgent);
	const holder = selected === null ? undefined : view.tables.agent.get(selected);
	const holdsSelected =
		project !== undefined &&
		holder !== undefined &&
		view.tables.workspace.get(holder.workspace)?.project === project.id;
	const action = cn(HEADER_ACTION, holdsSelected ? "opacity-100" : "opacity-0");

	const header = (
		<div className="group/header flex h-[22px] shrink-0 items-center">
			<button
				type="button"
				aria-expanded={open}
				onClick={toggle}
				className="flex h-full min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 text-left text-xs font-semibold text-muted-foreground hover:text-secondary-foreground"
			>
				<Chevron closed={!open} />
				<span className="min-w-0 flex-1 truncate text-terminal">{group.name}</span>
				<span className="shrink-0 truncate text-[11px] font-normal">
					{group.devices.join(", ")}
				</span>
			</button>
			{project ? (
				<button
					type="button"
					aria-label={t("New agent in {{name}}", { name: group.name })}
					title={t("New agent in {{name}}", { name: group.name })}
					onClick={() => openNewAgent(services, project.id)}
					className={action}
				>
					<svg
						aria-hidden
						width={12}
						height={12}
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						strokeWidth={2.2}
						strokeLinecap="round"
					>
						<path d="M12 5v14M5 12h14" />
					</svg>
				</button>
			) : null}
			{project ? (
				<Menu.Root>
					<Menu.Trigger
						aria-label={t("More for {{name}}", { name: group.name })}
						title={t("More")}
						className={cn(
							action,
							"data-popup-open:bg-accent data-popup-open:text-foreground data-popup-open:opacity-100"
						)}
					>
						<Ellipsis aria-hidden size={13} strokeWidth={2.2} />
					</Menu.Trigger>
					<Menu.Portal>
						<Menu.Positioner
							side="bottom"
							align="end"
							sideOffset={4}
							className="z-[130]"
						>
							<Menu.Popup className={PROJECT_MENU}>
								<ProjectMenuItems
									project={project}
									hidden={group.hidden}
									now={now}
								/>
							</Menu.Popup>
						</Menu.Positioner>
					</Menu.Portal>
				</Menu.Root>
			) : null}
		</div>
	);

	return (
		<section aria-label={group.name} className={cn("flex flex-col gap-px", !first && "mt-1.5")}>
			{project ? (
				<ProjectContextMenu project={project} hidden={group.hidden} now={now}>
					{header}
				</ProjectContextMenu>
			) : (
				header
			)}
			{open ? (
				<>
					{project ? <PullsEntry project={project} now={now} /> : null}
					{project ? <NotesEntry project={project} /> : null}
					{project ? <ProjectWorkflows project={project} now={now} /> : null}
					{group.agents.map((agent) => (
						<AgentRow
							key={agent.id}
							agent={agent}
							view={view}
							now={now}
							selected={lookingAt(agent)}
						/>
					))}
					{group.older.length > 0 ? (
						<>
							<button
								type="button"
								aria-expanded={showingOlder}
								onClick={() => toggleOlder(group.key)}
								className="flex h-[22px] shrink-0 items-center gap-1.5 rounded-md pr-2 pl-[22px] text-left text-[11.5px] text-muted-foreground hover:text-secondary-foreground"
							>
								<Chevron closed={!showingOlder} />
								{showingOlder
									? t("Hide {{length}} older", {
											length: formatNumber(group.older.length),
										})
									: t("{{length}} older", {
											length: formatNumber(group.older.length),
										})}
							</button>
							{showingOlder
								? group.older.map((agent) => (
										<AgentRow
											key={agent.id}
											agent={agent}
											view={view}
											now={now}
											selected={lookingAt(agent)}
										/>
									))
								: null}
						</>
					) : null}
					{group.agents.length === 0 &&
					group.older.length === 0 &&
					(!project ||
						(notesOf(view, project.id).length === 0 &&
							projectRuns(view, project.id).length === 0)) ? (
						<p className="flex h-[22px] items-center pl-[22px] text-[11.5px] text-muted-foreground">
							{t("No agents")}
						</p>
					) : null}
				</>
			) : null}
		</section>
	);
};

/** The green box listing every agent that waits on you. Hidden when none do. */
export const NeedsYou = () => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const items = needsYou(view);
	if (items.length === 0) return null;
	return (
		<section
			aria-label={t("{{countText}} agents need you", {
				count: items.length,
				countText: formatNumber(items.length),
			})}
			className="mx-1 mb-2.5 flex shrink-0 flex-col rounded-lg border border-status-ready/22 bg-status-ready/7 p-1.5"
		>
			<div
				title={t("Next agent waiting on you {{value1}}", {
					value1: keyHint(SHORTCUTS.nextWaiting),
				})}
				className="flex items-center justify-between pr-1 pb-1 pl-[11px] text-xs font-semibold text-status-ready-text"
			>
				<span>{t("Needs you")}</span>
				<span className="tabular-nums">{formatNumber(items.length)}</span>
			</div>
			{items.map((item, i) => (
				<button
					key={item.agent.id}
					type="button"
					onClick={() => openAgent(services, item.agent.id)}
					onPointerDown={(e) => pressAgent(services, item.agent, e.nativeEvent)}
					className={cn(
						"flex h-6 items-center gap-2 rounded-md px-1.5 text-left hover:bg-status-ready/12",
						i === 0 && "bg-status-ready/12"
					)}
				>
					<span
						aria-hidden
						className="size-[9px] shrink-0 animate-needs-you rounded-full bg-status-ready"
					/>
					<span className="min-w-0 flex-1 truncate text-[12.5px] text-foreground-strong">
						{item.agent.title}
					</span>
					<span className="shrink-0 text-[11px] text-status-ready-text">{item.ask}</span>
				</button>
			))}
		</section>
	);
};

/**
 * The pinned agents, every project with its active agents, the hidden
 * projects, and the shelves.
 */
export const ProjectList = ({ now }: { now: Timestamp }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const workingShelf = usePrefs((p) => p.workingShelf);
	const groups = projectGroups(view, now, { workingShelf });
	const sections = useMemo(
		() => inboxSections(view, now, { workingShelf }),
		[view, now, workingShelf]
	);
	const hidden = useMemo(() => hiddenProjectList(view), [view]);
	const query = useFilter();
	const words = useMemo(() => searchWords(query), [query]);
	const filtering = words.length > 0;
	const lists = useMemo(
		() => filterSidebar(view, now, { groups, sections, hidden }, words),
		[view, now, groups, sections, hidden, words]
	);
	const { pinned, working, snoozed, settled } = lists.sections;
	const shown = lists.groups.length + pinned.length + working.length + snoozed.length;
	if (shown + settled.length + lists.hidden.length === 0) {
		return (
			<p className="px-2 py-1 text-xs text-muted-foreground">
				{filtering
					? t("No agents match")
					: t("No agents yet. Start one in a terminal and it shows up here.")}
			</p>
		);
	}
	return (
		<div
			ref={rowFocus}
			role="listbox"
			aria-label={t("Agents")}
			aria-multiselectable
			className="flex shrink-0 grow flex-col gap-px"
		>
			<PinnedList agents={pinned} view={view} now={now} />
			{lists.groups.map((group, i) => (
				<Group key={group.key} group={group} view={view} now={now} first={i === 0} />
			))}
			{lists.groups.length === 0 && !filtering ? (
				<p className="px-2 py-1 text-xs text-muted-foreground">{t("No active agents")}</p>
			) : null}
			<HiddenProjects projects={lists.hidden} />
			<Shelves sections={lists.sections} view={view} now={now} />
		</div>
	);
};

// The agent rows take one Tab stop, and Up and Down move between them.
const rowFocus = (root: HTMLDivElement | null) => (root ? watchRowFocus(root) : undefined);
