// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Links that open a view of the app, such as maxhq://settings/open-in in a
// dispatcher's answer: a place in Settings, an agent, a project's settings
// or pull requests, a note, a device's page or a card of its settings, or
// the projects or devices view. A link to a Settings page from before
// opens where that page went.

import type { ComponentProps } from "react";

import type { AgentId, NoteId, ProjectId } from "@maxhq/shared/bindings";
import { deviceCardPlace, settingsPlace } from "@maxhq/shared/guide/settings-link";
import { parseViewLink, type ViewLink } from "@maxhq/shared/guide/view-link";
import { linkForDevice } from "@maxhq/shared/view/select";
import { openNote } from "@/notes/open";
import { openSettings } from "@/settings/store";

import { openAgent } from "./open-agent";
import { type Services, useServicesIfAny } from "./services";

/** Opens the view a link names. */
export const openViewLink = (services: Services, link: ViewLink): void => {
	switch (link.kind) {
		case "settings":
			return openSettings(services, settingsPlace(link.page));
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
			if (!found) return services.ui.getState().setMode("devices");
			if (link.card === null || found.device === null)
				return services.ui.getState().selectLink(found.id);
			return openSettings(services, deviceCardPlace(found.device, link.card));
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
