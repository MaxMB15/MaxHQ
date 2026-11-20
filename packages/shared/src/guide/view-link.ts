// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The links that open a view of the app, such as maxhq://settings/editor in
// a dispatcher's answer. VIEWS lists the views a link reaches, and the
// guide's map is made from it, so a new kind of link needs a guide section.

/** A view a link opens. */
export type ViewLink =
	| { readonly kind: "settings"; readonly page: string | null }
	| { readonly kind: "agent"; readonly agent: string }
	| { readonly kind: "project"; readonly project: string; readonly tab: ProjectTab }
	| { readonly kind: "note"; readonly project: string; readonly note: string }
	| { readonly kind: "device"; readonly device: string; readonly card: string | null }
	| { readonly kind: "mode"; readonly mode: "projects" | "devices" };

/** Where a project's link opens: its settings, a tab of them, or its pull requests. */
export type ProjectTab = "general" | "skills" | "pulls";

/** What every view link starts with. */
const VIEW_LINK = "maxhq://";

/** A view a link reaches, with its link. Braces mark what the link fills in. */
export type ViewEntry = {
	readonly id: string;
	readonly title: string;
	readonly link: string;
};

/** The views links reach, in the order the guide's map lists them. */
export const VIEWS: readonly ViewEntry[] = [
	{ id: "view/projects", title: "Projects", link: "maxhq://view/projects" },
	{ id: "view/devices", title: "Devices", link: "maxhq://view/devices" },
	{ id: "view/agent", title: "An agent", link: "maxhq://agent/{agent}" },
	{
		id: "view/pulls",
		title: "A project's pull requests",
		link: "maxhq://project/{project}/pulls",
	},
	{ id: "view/note", title: "A note", link: "maxhq://note/{project}/{note}" },
	{ id: "view/device", title: "A device's page", link: "maxhq://device/{device}" },
	{ id: "view/settings", title: "Settings", link: "maxhq://settings" },
];

const TABS: readonly ProjectTab[] = ["general", "skills", "pulls"];

/** Whether a link is one of the app's own. */
export const isViewLink = (href: string | undefined): href is `${typeof VIEW_LINK}${string}` =>
	href?.startsWith(VIEW_LINK) === true;

/** The view a link opens, or null when it isn't one the app knows. */
export const parseViewLink = (href: string): ViewLink | null => {
	if (!isViewLink(href)) return null;
	let parts: string[];
	try {
		parts = href.slice(VIEW_LINK.length).split("/").map(decodeURIComponent);
	} catch {
		return null;
	}
	if (parts.at(-1) === "") parts.pop();
	if (parts.some((part) => part === "")) return null;
	const [kind, first, second, ...rest] = parts;
	if (rest.length > 0) return null;
	switch (kind) {
		case "settings":
			return second === undefined ? { kind, page: first ?? null } : null;
		case "agent":
			return first !== undefined && second === undefined ? { kind, agent: first } : null;
		case "project": {
			const tab = (second ?? "general") as ProjectTab;
			return first !== undefined && TABS.includes(tab) ? { kind, project: first, tab } : null;
		}
		case "note":
			return first !== undefined && second !== undefined
				? { kind, project: first, note: second }
				: null;
		case "device":
			return first !== undefined ? { kind, device: first, card: second ?? null } : null;
		case "view":
			return (first === "projects" || first === "devices") && second === undefined
				? { kind: "mode", mode: first }
				: null;
		default:
			return null;
	}
};
