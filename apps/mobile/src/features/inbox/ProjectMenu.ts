// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a project's menu offers, from the ⋯ on its group in the agent list,
// as the desktop's project menu does: its notes, a new note while a device
// can take one, its pull requests where a GitHub App reaches it, the
// scheduled tasks, hiding or showing it, and its settings. Hiding needs no
// switch: it only changes what the lists show.

import { t } from "@maxhq/shared/i18n";

import type { Project } from "@maxhq/shared/bindings";
import { appReaches } from "@maxhq/shared/pulls/app";
import { setProjectHidden } from "@maxhq/shared/ui/hide-project";
import { notesEntry, remoteNotesReach } from "@maxhq/shared/view/notes";
import { canHideProjects } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import type { Services } from "../../services";
import { type Choice, choose } from "../../ui/choose";
import { openNewNote } from "../notes/NotesScreen";
import { noticesOf } from "./notices";

type Navigation = ScreenProps<"Inbox">["navigation"];

/** The menu's entries, top to bottom; `hidden` is whether you hid the project. */
export const projectMenu = (
	services: Services,
	navigation: Navigation,
	project: Project,
	hidden: boolean
): Choice[] => {
	const { view } = services.view.getState();
	const id = project.id;
	const notes = notesEntry(view, id, remoteNotesReach(view, id));
	const choices: Choice[] = [
		{ label: t("Notes"), pick: () => navigation.navigate("Notes", { project: id }) },
	];
	if (notes?.writable) {
		choices.push({
			label: t("New note"),
			pick: () =>
				void openNewNote(services, id, (note) =>
					navigation.navigate("Note", { project: id, note, made: true })
				),
		});
	}
	if (appReaches(view, project, Date.now())) {
		choices.push({
			label: t("Pull requests"),
			pick: () => navigation.navigate("Pulls", { project: id }),
		});
	}
	choices.push({ label: t("Scheduled tasks"), pick: () => navigation.navigate("Tasks") });
	if (canHideProjects(view)) {
		choices.push({
			label: hidden ? t("Show project") : t("Hide project"),
			pick: () => void setProjectHidden(noticesOf(services), project, !hidden),
		});
	}
	choices.push({
		label: t("Project settings…"),
		pick: () => navigation.navigate("ProjectSettings", { project: id }),
	});
	return choices;
};

/** Opens the project's menu in the system's sheet. */
export const openProjectMenu = (
	services: Services,
	navigation: Navigation,
	project: Project,
	hidden: boolean
): void => choose(project.name, projectMenu(services, navigation, project, hidden));

/** Opens the hidden projects, each with a way to show it again. */
export const openHiddenProjects = (services: Services, projects: readonly Project[]): void =>
	choose(
		t("Hidden projects"),
		projects.map((project) => ({
			label: t("Show project {{name}}", { name: project.name }),
			pick: () => void setProjectHidden(noticesOf(services), project, false),
		}))
	);
