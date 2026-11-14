// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Each note at its newest report. Every device that holds a project's
// notes reports them, and the lists differ only until the devices sync, so
// the report with the latest head wins, and of two at once, the device
// whose ID sorts first. It's apart from `./notes` so `./select` can ask
// which projects have notes without the two importing each other.

import type { DeviceId, NoteId, NoteSummary, ProjectId, ProjectNotes } from "../bindings";
import { all, type ViewState } from "./replica";

/** A note's newest report, and the device it came from. */
export type NoteHead = { readonly summary: NoteSummary; readonly from: DeviceId };

/** Every device's report of a project's notes. */
export const reportsOf = (view: ViewState, project: ProjectId): ProjectNotes[] =>
	all(view, "project_notes").filter((list) => list.project === project);

/** Each note's newest report among `reports`, tombstones kept. */
export const newestNotes = (reports: readonly ProjectNotes[]): Map<NoteId, NoteHead> => {
	const out = new Map<NoteId, NoteHead>();
	for (const report of reports) {
		for (const summary of report.notes) {
			const held = out.get(summary.note);
			const wins =
				!held ||
				summary.at > held.summary.at ||
				(summary.at === held.summary.at && report.device < held.from);
			if (wins) out.set(summary.note, { summary, from: report.device });
		}
	}
	return out;
};

/** The projects with at least one live note on some device. */
export const projectsWithNotes = (view: ViewState): ReadonlySet<ProjectId> => {
	const byProject = new Map<ProjectId, ProjectNotes[]>();
	for (const list of all(view, "project_notes")) {
		byProject.set(list.project, [...(byProject.get(list.project) ?? []), list]);
	}
	const out = new Set<ProjectId>();
	for (const [project, reports] of byProject) {
		for (const { summary } of newestNotes(reports).values()) {
			if (summary.deleted) continue;
			out.add(project);
			break;
		}
	}
	return out;
};
