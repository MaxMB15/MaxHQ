// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The devices side of the sidebar, grouped as decision N-5 says: this Mac,
// then each network, then each device on it, then the device's container
// workspaces. A device row shows its own connection dot and, beside the
// name, the worst dot among its agents. A workspace row shows how much
// unsaved work its guest last read.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import {} from "react";

import { useUi, useView } from "@/app/services";
import { ConnectionDot } from "@/components/status/dots";
import { DeviceConnectionDot } from "@/components/status/agent-dots";
import type { LinkId, Timestamp, Workspace } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { unsavedOf } from "@maxhq/shared/hosts/unsaved";
import { CONNECTION_LABEL } from "@maxhq/shared/status/connection";
import { plural } from "@maxhq/shared/status/format";
import { heldLabel } from "@maxhq/shared/ui/link-claim";
import { type DeviceGroup, type DeviceNode, deviceGroups } from "@maxhq/shared/view/select";

/** What a device row says on its right, and in which tone. */
export type DeviceRowStatus = {
	readonly text: string;
	readonly tone: "plain" | "error" | "connecting";
};

/** The device row's right side: its agent count when connected, or what's wrong. */
export const deviceRowStatus = (node: DeviceNode): DeviceRowStatus => {
	const held = heldLabel(node.link);
	if (held) {
		return { text: held, tone: node.link.claim?.state === "asking" ? "connecting" : "error" };
	}
	const state = node.link.state;
	switch (state.state) {
		case "connected":
			return { text: plural(node.agents.length, "agent"), tone: "plain" };
		case "connecting":
			switch (state.step) {
				case "installing_maxd":
					return { text: t("Installing maxd"), tone: "connecting" };
				case "waiting_for_reboot":
					return { text: t("Restarting"), tone: "connecting" };
				case "waiting_for_approval":
					return { text: t("Waiting for approval"), tone: "connecting" };
				default:
					return { text: t("Connecting"), tone: "connecting" };
			}
		case "unreachable":
			return {
				text: node.link.peer && !node.link.peer.online ? t("Offline") : t("Not reachable"),
				tone: "error",
			};
		case "unknown":
			return { text: t("Not checked yet"), tone: "plain" };
	}
};

const TONE: Record<DeviceRowStatus["tone"], string> = {
	plain: "text-muted-foreground",
	error: "text-status-error-text",
	connecting: "text-status-connecting-text",
};

const LockIcon = () => {
	useI18n();
	return (
		<svg
			aria-hidden
			width={9}
			height={9}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={2.6}
			strokeLinecap="round"
			strokeLinejoin="round"
		>
			<rect x="5" y="11" width="14" height="10" rx="2" />
			<path d="M8 11V7a4 4 0 0 1 8 0v4" />
		</svg>
	);
};

/** The count of a workspace's unsaved work, when its guest read any. */
export const UnsavedBadge = ({ workspace, now }: { workspace: Workspace; now: Timestamp }) => {
	const { t } = useI18n();

	const unsaved = unsavedOf(workspace, now);
	if (!unsaved?.any) return null;
	const words = t("Unsaved: {{text}}. {{readAt}}.", {
		text: unsaved.text,
		readAt: unsaved.readAt,
	});
	return (
		<span
			role="img"
			aria-label={words}
			title={words}
			className={cn(
				"inline-flex h-4 shrink-0 items-center rounded-[4px] px-[5px] text-[10.5px] font-semibold tabular-nums",
				unsaved.old
					? "bg-border text-muted-foreground"
					: "bg-status-connecting/16 text-status-connecting-text"
			)}
		>
			{formatNumber(unsaved.count)}
		</span>
	);
};

const DeviceRow = ({
	node,
	now,
	selected,
}: {
	node: DeviceNode;
	now: Timestamp;
	selected: boolean;
}) => {
	const { t } = useI18n();

	const selectLink = useUi((s) => s.selectLink);
	const status = deviceRowStatus(node);
	return (
		<>
			<button
				type="button"
				aria-current={selected ? "true" : undefined}
				onClick={() => selectLink(node.link.id)}
				className={cn(
					"flex h-[30px] w-full shrink-0 items-center gap-2 rounded-md px-2 text-left",
					selected
						? "bg-selected shadow-[inset_0_0_0_1px_var(--selected-ring)]"
						: "hover:bg-accent"
				)}
			>
				<DeviceConnectionDot ctx={node.ctx} now={now} />
				<span
					className={cn(
						"min-w-0 flex-1 truncate text-[12.5px]",
						selected ? "text-foreground-strong" : "text-foreground"
					)}
				>
					{node.link.name}
				</span>
				{node.worst && node.look === "live" ? (
					<ConnectionDot
						look={node.worst}
						size={6}
						label={t("Worst agent: {{value1}}", {
							value1: CONNECTION_LABEL[node.worst].toLowerCase(),
						})}
					/>
				) : null}
				<span className={cn("shrink-0 text-[11px]", TONE[status.tone])}>{status.text}</span>
			</button>
			{node.workspaces.map((ws) => (
				<button
					key={ws.workspace.id}
					type="button"
					onClick={() => selectLink(node.link.id)}
					className="flex h-6 shrink-0 items-center gap-2 rounded-md pr-2 pl-[26px] text-left text-xs text-secondary-foreground hover:bg-accent"
				>
					<ConnectionDot look={ws.worst ?? node.look} size={8} decorative />
					<span className="min-w-0 flex-1 truncate">{ws.name}</span>
					<UnsavedBadge workspace={ws.workspace} now={now} />
					<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
						{formatNumber(ws.agents.length)}
					</span>
				</button>
			))}
		</>
	);
};

const Group = ({
	group,
	now,
	first,
	shown,
}: {
	group: DeviceGroup;
	now: Timestamp;
	first: boolean;
	shown: LinkId | undefined;
}) => {
	const { t } = useI18n();
	return (
		<section aria-label={group.name} className={cn("flex flex-col gap-px", !first && "mt-1.5")}>
			<div className="flex h-[22px] shrink-0 items-center gap-1.5 px-2 text-xs font-semibold text-terminal">
				<span className="min-w-0 truncate">{group.name}</span>
				{group.network?.kind.type === "tailscale" ? (
					<span
						title={t("Reaching these devices needs Tailscale on this Mac")}
						className="inline-flex h-4 shrink-0 items-center gap-[3px] rounded-[4px] bg-border px-[5px] text-[10.5px] font-medium text-secondary-foreground"
					>
						<LockIcon />
						VPN
					</span>
				) : null}
			</div>
			{group.devices.map((node) => (
				<DeviceRow
					key={node.link.id}
					node={node}
					now={now}
					selected={node.link.id === shown}
				/>
			))}
		</section>
	);
};

/**
 * The device the devices view shows: the one you picked, else the first
 * with a problem, else the first.
 */
export const shownDevice = (
	nodes: readonly DeviceNode[],
	selected: LinkId | null
): DeviceNode | undefined =>
	nodes.find((n) => n.link.id === selected) ?? nodes.find((n) => n.look !== "live") ?? nodes[0];

/** Every device, grouped by how it's reached. */
export const DeviceList = ({ now }: { now: Timestamp }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const selected = useUi((s) => s.selectedLink);
	const groups = deviceGroups(view, now);
	const shown = shownDevice(
		groups.flatMap((g) => g.devices),
		selected
	)?.link.id;
	if (groups.length === 0) {
		return <p className="px-2 py-1 text-xs text-muted-foreground">{t("No devices yet.")}</p>;
	}
	return (
		<>
			{groups.map((group, i) => (
				<Group key={group.key} group={group} now={now} first={i === 0} shown={shown} />
			))}
		</>
	);
};
