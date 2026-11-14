// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An agent's two dots with their hovers, and a device's dot with its hover.
// Each reads what its card needs from the view, so a row only passes the
// agent or the device.

import type { Agent, Timestamp } from "@maxhq/shared/bindings";
import { activityCard, agentLook } from "@maxhq/shared/status/activity";
import {
	type LinkContext,
	agentConnection,
	deviceConnection,
} from "@maxhq/shared/status/connection";
import { agentLinkContext, attentionOf, childrenOf } from "@maxhq/shared/view/select";
import type { ViewState } from "@maxhq/shared/view/replica";

import { ActivityDot, ConnectionDot, type ConnectionDotSize } from "./dots";
import { StatusHover } from "./status-hover";

/** The activity dot for an agent, with its hover. */
export const AgentActivityDot = ({
	agent,
	view,
	now,
	className,
}: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	className?: string;
}) => {
	const look = agentLook(agent, now);
	return (
		<StatusHover
			className={className}
			dot={<ActivityDot look={look} decorative />}
			card={(at) =>
				activityCard({
					agent,
					account: agent.account ? view.tables.account.get(agent.account) : undefined,
					attention: attentionOf(view, agent),
					children: childrenOf(view, agent.id),
					now: at,
				})
			}
		>
			<ActivityDot look={look} />
		</StatusHover>
	);
};

/** The connection dot for an agent, with its hover. */
export const AgentConnectionDot = ({
	agent,
	view,
	now,
	size = 6,
}: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	size?: ConnectionDotSize;
}) => {
	const ctx = agentLinkContext(view, agent);
	const { look } = agentConnection(agent, ctx, now);
	return (
		<StatusHover
			dot={<ConnectionDot look={look} size={10} decorative />}
			card={(at) => agentConnection(agent, ctx, at).card}
		>
			<ConnectionDot look={look} size={size} />
		</StatusHover>
	);
};

/** The connection dot for a device, with its hover. */
export const DeviceConnectionDot = ({
	ctx,
	now,
	size = 10,
}: {
	ctx: LinkContext;
	now: Timestamp;
	size?: ConnectionDotSize;
}) => {
	const { look } = deviceConnection(ctx, now);
	return (
		<StatusHover
			dot={<ConnectionDot look={look} size={10} decorative />}
			card={(at) => deviceConnection(ctx, at).card}
		>
			<ConnectionDot look={look} size={size} />
		</StatusHover>
	);
};
