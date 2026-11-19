// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The container workspaces MaxHQ made, as a card for a device's detail or a
// project's settings, with the buttons that make another and copy a project
// there.

import { useI18n } from "@maxhq/shared/i18n";

import { useServices, useView } from "@/app/services";
import type { DeviceId, ProjectId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { SECONDARY, Section } from "@/settings/controls";
import { deviceName } from "@maxhq/shared/view/select";

import { type CopyRequest, openCopyProject } from "./copy-store";
import { NewWorkspaceButton } from "./new-workspace";
import { copySources } from "@maxhq/shared/workspaces/copy";
import { deviceWorkspaces } from "@maxhq/shared/workspaces/device";
import { canCopy, madeWorkspaces, workspaceHosts } from "@maxhq/shared/workspaces/state";
import { WorkspaceCard } from "./workspace-card";

/** Opens the copy form for a project or onto a device. */
const CopyButton = ({ request, disabled }: { request: CopyRequest; disabled: boolean }) => {
	const { t } = useI18n();

	const services = useServices();
	return (
		<button
			type="button"
			className={SECONDARY}
			disabled={disabled}
			onClick={() => openCopyProject(services, request)}
		>
			{t("Copy to another device…")}
		</button>
	);
};

/** A device's workspaces, with a note on the containers MaxHQ leaves alone. */
export const DeviceWorkspaces = ({ device }: { device: DeviceId }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(5000);
	const section = deviceWorkspaces(view, device, now);
	if (!section) return null;
	const name = deviceName(view, device);
	const { made, makes, live } = section;
	return (
		<Section
			title={t("Workspaces on {{name}}", { name: name })}
			label={t("Workspaces on {{name}}", { name: name })}
			hint={section.hint}
		>
			{made.length > 0 ? (
				<div>
					{made.map((w) => (
						<WorkspaceCard key={w.id} workspace={w} />
					))}
				</div>
			) : (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					{t(
						"No workspaces yet. Make one for a project and its agents run in a container here."
					)}
				</p>
			)}
			<div className="flex items-center gap-3 border-t border-sidebar-border px-4 py-3">
				<span className="min-w-0 flex-1 text-xs text-muted-foreground">
					{section.footer}
				</span>
				<CopyButton request={{ device }} disabled={!live || !canCopy(view, device)} />
				{makes ? <NewWorkspaceButton device={device} disabled={!live} /> : null}
			</div>
		</Section>
	);
};

/** A project's workspaces on every device. */
export const ProjectWorkspaces = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const made = madeWorkspaces(view, { project });
	const hosts = workspaceHosts(view);
	const name = view.tables.project.get(project)?.name ?? project;
	const copies =
		!!view.tables.project.get(project)?.repo && copySources(view, project).length > 0;
	return (
		<Section
			title={t("Workspaces")}
			label={t("Workspaces for {{name}}", { name: name })}
			hint={t("Containers on devices with Incus")}
		>
			{made.length > 0 ? (
				<div>
					{made.map((w) => (
						<WorkspaceCard key={w.id} workspace={w} showProject={false} showDevice />
					))}
				</div>
			) : (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					{hosts.length > 0
						? t("No workspaces for {{name}} yet.", { name: name })
						: t("No device has Incus, so there's nowhere to make one.")}
				</p>
			)}
			<div className="flex justify-end gap-2 border-t border-sidebar-border px-4 py-3">
				<CopyButton request={{ project }} disabled={!copies} />
				<NewWorkspaceButton project={project} disabled={hosts.length === 0} />
			</div>
		</Section>
	);
};
