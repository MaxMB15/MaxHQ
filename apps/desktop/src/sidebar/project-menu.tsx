// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a project's menu offers, from the ⋯ on its header or a right-click
// anywhere on it. New agent does what the + on the header does. Pull
// requests is here for a project whose entry doesn't show, because it has
// no PRs yet, and only where a GitHub App reaches it. New note is here
// while this Mac can write the project's notes, as it is on the Notes
// entry's right-click.

import { useI18n } from "@maxhq/shared/i18n";

import { ContextMenu } from "@base-ui/react/context-menu";
import { Menu } from "@base-ui/react/menu";
import {
	CopyPlus,
	Eye,
	EyeOff,
	GitPullRequest,
	type LucideIcon,
	NotebookPen,
	Plus,
	Settings,
} from "lucide-react";
import type { ReactElement, ReactNode } from "react";

import { type Services, useServices, useUi, useView } from "@/app/services";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { cn } from "@/lib/utils";
import { openNewAgent } from "@/new-agent/store";
import { createNote } from "@/notes/queries";
import { openSettings } from "@/settings/store";
import { openCopyProject } from "@/workspaces/copy-store";
import type { Project, ProjectId, Timestamp } from "@maxhq/shared/bindings";
import { appReaches } from "@maxhq/shared/pulls/app";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";
import { notesReach } from "@maxhq/shared/view/notes";
import { canHideProjects } from "@maxhq/shared/view/select";

import { setHidden } from "./hidden-projects";
import { noticesOf } from "./notices";

/** The look of a project's menu. */
export const PROJECT_MENU = cn(MENU_POPUP, "w-[220px]");

/** One item of a sidebar menu. */
export const MenuItem = ({
	icon: Icon,
	label,
	onClick,
}: {
	icon: LucideIcon;
	label: string;
	onClick: () => void;
}) => {
	useI18n();
	return (
		<Menu.Item onClick={onClick} className={MENU_ITEM}>
			<Icon
				aria-hidden
				size={13}
				strokeWidth={2}
				className="shrink-0 text-secondary-foreground"
			/>
			<span className="text-[12.5px] font-[550]">{label}</span>
		</Menu.Item>
	);
};

/** Makes a note in `project` and opens it, or says under the list why it couldn't. */
const startNote = (services: Services, project: ProjectId) =>
	void createNote(services, project).then((failure) => {
		if (failure !== null) showFailure(noticesOf(services), failure);
	});

/** New note, the same in every menu that offers it. */
export const NewNoteItem = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();

	const services = useServices();
	return (
		<MenuItem
			icon={NotebookPen}
			label={t("New note")}
			onClick={() => startNote(services, project)}
		/>
	);
};

/** The menu's items, the same from the ⋯ and from a right-click. */
export const ProjectMenuItems = ({
	project,
	hidden,
	now,
}: {
	project: Project;
	hidden: boolean;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const openPulls = useUi((s) => s.openPulls);
	return (
		<>
			<MenuItem
				icon={Plus}
				label={t("New agent")}
				onClick={() => openNewAgent(services, project.id)}
			/>
			{appReaches(view, project, now) ? (
				<MenuItem
					icon={GitPullRequest}
					label={t("Pull requests")}
					onClick={() => openPulls(project.id)}
				/>
			) : null}
			{notesReach(view, project.id).readOnly ? null : <NewNoteItem project={project.id} />}
			<MenuItem
				icon={CopyPlus}
				label={t("Copy to another device…")}
				onClick={() => openCopyProject(services, { project: project.id })}
			/>
			{canHideProjects(view) ? (
				<MenuItem
					icon={hidden ? Eye : EyeOff}
					label={hidden ? t("Show project") : t("Hide project")}
					onClick={() => void setHidden(services, project, !hidden)}
				/>
			) : null}
			<MenuItem
				icon={Settings}
				label={t("Project settings…")}
				onClick={() => openSettings(services, { kind: "project", project: project.id })}
			/>
		</>
	);
};

/** Opens `items` as a menu from a right-click on `children`, or leaves it be with none. */
export const RightClickMenu = ({
	items,
	children,
}: {
	items: ReactNode;
	children: ReactElement;
}) => {
	useI18n();
	return items === null ? (
		children
	) : (
		<ContextMenu.Root>
			<ContextMenu.Trigger render={children} />
			<ContextMenu.Portal>
				<ContextMenu.Positioner className="z-[130]">
					<ContextMenu.Popup className={PROJECT_MENU}>{items}</ContextMenu.Popup>
				</ContextMenu.Positioner>
			</ContextMenu.Portal>
		</ContextMenu.Root>
	);
};

/** Opens the project's menu from a right-click on `children`. */
export const ProjectContextMenu = ({
	project,
	hidden,
	now,
	children,
}: {
	project: Project;
	hidden: boolean;
	now: Timestamp;
	children: ReactElement;
}) => {
	useI18n();
	return (
		<RightClickMenu items={<ProjectMenuItems project={project} hidden={hidden} now={now} />}>
			{children}
		</RightClickMenu>
	);
};
