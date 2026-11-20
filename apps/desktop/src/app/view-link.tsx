// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Links that open a view of the app, such as maxhq://settings/editor in a
// dispatcher's answer: a Settings page, an agent, a project's settings or
// pull requests, a note, a device's page, or the projects or devices view.

import type { ComponentProps } from "react";

import type { AgentId, NoteId, ProjectId } from "@maxhq/shared/bindings";
import { parseViewLink, type ViewLink } from "@maxhq/shared/guide/view-link";
import { LOGIN_PAGES } from "@maxhq/shared/settings/login-steps";
import { linkForDevice } from "@maxhq/shared/view/select";
import { openNote } from "@/notes/open";
import { appPages } from "@/settings/settings";
import { openSettings, type SettingsPage, showDeviceCard } from "@/settings/store";

import { openAgent } from "./open-agent";
import { type Services, useServicesIfAny } from "./services";

/** The Settings page a link names, or none for the one you left. */
const settingsPage = (page: string | null): SettingsPage | undefined => {
	const login = LOGIN_PAGES.find((name) => name === page);
	if (login) return { kind: "login", login };
	const app = appPages().find((each) => each.kind === page);
	return app && { kind: app.kind };
};

/** Opens the view a link names. */
export const openViewLink = (services: Services, link: ViewLink): void => {
	switch (link.kind) {
		case "settings":
			return openSettings(services, settingsPage(link.page));
		case "agent":
			return openAgent(services, link.agent as AgentId);
		case "project": {
			const project = link.project as ProjectId;
			if (link.tab === "pulls") return services.ui.getState().openPulls(project);
			return openSettings(services, { kind: "project", project, tab: link.tab });
		}
		case "note":
			return openNote(services, link.project as ProjectId, link.note as NoteId);
		case "device": {
			const found = linkForDevice(services.view.getState().view, link.device);
			if (found) return showDeviceCard(services, found.id, link.card);
			return services.ui.getState().setMode("devices");
		}
		case "mode":
			return services.ui.getState().setMode(link.mode);
	}
};

/** A link in Markdown to one of the app's views, which opens it in this window. */
export const ViewLinkAnchor = ({
	href,
	onClick,
	...props
}: ComponentProps<"a"> & { href: string }) => {
	const services = useServicesIfAny();
	const link = parseViewLink(href);
	return (
		<a
			{...props}
			href={href}
			onClick={(event) => {
				event.preventDefault();
				onClick?.(event);
				if (services && link) openViewLink(services, link);
			}}
		/>
	);
};
