// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The connection dot from decision N-2. A device's dot comes from its link.
// An agent's dot is its device's, except that a connected device with an
// agent that doesn't run shows purple, and one whose idle CLI `maxd`
// stopped shows asleep. The hover names where the chain from this Mac
// stops, using the checks below.

import { t } from "../i18n";

import type {
	Agent,
	AgentLink,
	Device,
	DeviceLink,
	Network,
	NetworkState,
	Timestamp,
} from "../bindings";

import { type ClaimNames, heldConnection } from "../ui/link-claim";

import type { Check, CheckId, CheckState, Fact, HoverCard } from "./card";
import { formatAgo, formatWhen, ordinal, plural, sentence } from "./format";

/** How the connection dot looks. */
export type ConnectionLook =
	"live" | "asleep" | "connecting" | "agent_down" | "unreachable" | "unknown";

/**
 * Worst first, for the second dot on a device row. Asleep is no trouble,
 * so it shows there only once every agent sleeps.
 */
const SEVERITY: Record<ConnectionLook, number> = {
	unreachable: 5,
	agent_down: 4,
	connecting: 3,
	unknown: 2,
	live: 1,
	asleep: 0,
};

/** The worst of several looks, or undefined when there are none. */
export const worstLook = (looks: readonly ConnectionLook[]): ConnectionLook | undefined =>
	looks.reduce<ConnectionLook | undefined>(
		(worst, look) => (worst === undefined || SEVERITY[look] > SEVERITY[worst] ? look : worst),
		undefined
	);

/** The words a screen reader hears for each look. */
export const CONNECTION_LABEL: Record<ConnectionLook, string> = {
	get live() {
		return t("Live");
	},
	get asleep() {
		return t("Asleep");
	},
	get connecting() {
		return t("Connecting");
	},
	get agent_down() {
		return t("Device up, agent not running");
	},
	get unreachable() {
		return t("Not reachable");
	},
	get unknown() {
		return t("Unknown");
	},
};

/** A connection dot and its hover. */
export type Connection = {
	/** How the dot looks. */
	readonly look: ConnectionLook;
	/** What the hover says. */
	readonly card: HoverCard;
};

/** What the connection helpers need to know about a device. */
export type LinkContext = {
	/** The client's connection to the device. */
	readonly link: DeviceLink;
	/** The device, once its `maxd` has answered. */
	readonly device?: Device;
	/** The network the route goes over, when there is one. */
	readonly network?: Network;
	/** The links for the hosts in `link.via`, nearest first, where the client has them. */
	readonly jumpHosts?: readonly DeviceLink[];
	/** Every agent on the device, sub-agents included. */
	readonly agents?: readonly Agent[];
	/** The names in the question the link waits on, while it's held. */
	readonly claimed?: ClaimNames;
};

// A device is never asleep: only its agents are.
const lookOfLink = (link: DeviceLink): Exclude<ConnectionLook, "asleep"> => {
	switch (link.state.state) {
		case "connected":
			return "live";
		case "connecting":
			return "connecting";
		case "unreachable":
			return "unreachable";
		case "unknown":
			return "unknown";
	}
};

/**
 * The address a phone's network route reaches, such as "192.168.1.20" for
 * "wss://192.168.1.20:7466", or null for any other route.
 */
export const networkAddress = (host: string): string | null => {
	const match = /^wss:\/\/(?:\[([^\]]+)\]|([^:/]+))/.exec(host);
	return match ? (match[1] ?? match[2] ?? null) : null;
};

const routeText = (link: DeviceLink): string => {
	if (link.route.type === "local") return t("This Mac");
	const address = networkAddress(link.route.host);
	return address === null
		? t("SSH as {{link_route_host}}", { link_route_host: link.route.host })
		: t("Network, {{address}}", { address: address });
};

const NETWORK_DETAIL: Record<NetworkState, { state: CheckState; detail: string }> = {
	up: {
		state: "passed",
		get detail() {
			return t("Running, signed in");
		},
	},
	starting: {
		state: "running",
		get detail() {
			return t("Starting up");
		},
	},
	off: {
		state: "failed",
		get detail() {
			return t("Tailscale is off on this Mac");
		},
	},
	needs_login: {
		state: "failed",
		get detail() {
			return t("Tailscale wants you to log in again");
		},
	},
	needs_approval: {
		state: "failed",
		get detail() {
			return t("An admin has to approve this Mac on the tailnet");
		},
	},
	not_running: {
		state: "failed",
		get detail() {
			return t("Tailscale isn't running on this Mac");
		},
	},
	unknown: {
		state: "unknown",
		get detail() {
			return t("Not checked yet");
		},
	},
};

const check = (
	id: CheckId,
	short: string,
	title: string,
	state: CheckState,
	detail: string
): Check => ({
	id,
	short,
	title,
	state,
	detail,
});

/** What a check with no reading of its own says once a later one passed. */
const IMPLIED: Record<CheckId, string> = {
	get net() {
		return t("Online");
	},
	get tailnet() {
		return t("Running");
	},
	get peer() {
		return t("Reachable");
	},
	get ping() {
		return t("Answers");
	},
	get ssh() {
		return t("Connected");
	},
	get maxd() {
		return t("Answered");
	},
	get agents() {
		return t("Running");
	},
};

/**
 * The chain of checks from this Mac to the device's agents, in the order
 * they run. Each check runs only if the one above passed, so everything
 * after a failure reads "Not run". A check with no reading of its own
 * passed when a later one did.
 */
export const linkChecks = (ctx: LinkContext, now: Timestamp): Check[] => {
	const { link, device, network, agents = [] } = ctx;
	const name = link.name;
	const local = link.route.type === "local";
	const host = link.route.type === "ssh" ? link.route.host : name;
	const state = link.state;
	const checks: Check[] = [];

	if (!local)
		checks.push(
			check("net", t("Net"), t("This Mac's network"), "unknown", t("Not checked yet"))
		);

	if (!local && link.network !== null) {
		const reading = network ? NETWORK_DETAIL[network.state] : NETWORK_DETAIL.unknown;
		const version =
			network?.state === "up" && network.kind.client_version
				? t("Running {{network_kind_client_version}}, signed in", {
						network_kind_client_version: network.kind.client_version,
					})
				: reading.detail;
		checks.push(
			check("tailnet", t("Tailnet"), t("Tailscale on this Mac"), reading.state, version)
		);

		const peer = link.peer;
		let peerState: CheckState = "unknown";
		let peerDetail = t("Not checked yet");
		if (peer?.online) {
			peerState = "passed";
			peerDetail =
				peer.path.type === "direct"
					? t("Online, direct at {{peer_path_address}}", {
							peer_path_address: peer.path.address,
						})
					: peer.path.type === "relayed"
						? t("Online, relayed through {{peer_path_region}}", {
								peer_path_region: peer.path.region,
							})
						: t("Online");
		} else if (peer) {
			peerState = "failed";
			peerDetail = peer.last_seen
				? t(
						"Offline since {{formatWhen_peer_last_seen__now}}. Tailscale says the peer is gone, so the checks below can't run",
						{ formatWhen_peer_last_seen__now: formatWhen(peer.last_seen, now) }
					)
				: t("Offline. Tailscale says the peer is gone, so the checks below can't run");
		}
		checks.push(
			check(
				"peer",
				t("Peer"),
				t("{{name}} on the tailnet", { name: name }),
				peerState,
				peerDetail
			)
		);
	}

	if (!local) checks.push(check("ping", t("Ping"), t("Ping"), "unknown", t("Not checked yet")));

	let sshState: CheckState = "unknown";
	let sshDetail = t("Not checked yet");
	let maxdState: CheckState = "unknown";
	let maxdDetail = t("Not checked yet");
	switch (state.state) {
		case "connected":
			sshState = "passed";
			sshDetail = t("Connected as {{host}}", { host: host });
			maxdState = "passed";
			maxdDetail = device
				? t("Answered, maxd {{device_maxd_version}}", {
						device_maxd_version: device.maxd_version,
					})
				: t("Answered");
			break;
		case "connecting":
			if (state.step === "dialing") {
				sshState = "running";
				sshDetail = t("Connecting as {{host}}. {{ordinal_state_attempt}} try", {
					host: host,
					ordinal_state_attempt: ordinal(state.attempt),
				});
				maxdState = "not_run";
				maxdDetail = t("Not run");
			} else if (state.step === "waiting_for_reboot") {
				sshState = "running";
				sshDetail = t("Waiting for {{name}} to come back from a restart", { name: name });
				maxdState = "not_run";
				maxdDetail = t("Not run");
			} else {
				sshState = "passed";
				sshDetail = t("Connected as {{host}}", { host: host });
				maxdState = "running";
				maxdDetail =
					state.step === "installing_maxd"
						? t("Installing maxd")
						: state.step === "waiting_for_approval"
							? t("Waiting for you to let this app in")
							: t("Saying hello to maxd");
			}
			break;
		case "unreachable":
			sshState = "failed";
			sshDetail = state.last_seen
				? t("{{value1}}. Last worked at {{value2}} as {{value3}}", {
						value1: sentence(state.reason),
						value2: formatWhen(state.last_seen, now),
						value3: host,
					})
				: `${sentence(state.reason)}`;
			maxdState = "not_run";
			break;
		case "unknown":
			break;
	}
	if (local) {
		// The local route has no SSH leg, so its one check is maxd itself.
		const maxdLocal: CheckState = sshState === "failed" ? "failed" : maxdState;
		const detail =
			state.state === "unreachable"
				? sentence(state.reason)
				: state.state === "connected"
					? t("Answered on the local socket{{device______maxd___device_maxd_version}}", {
							device______maxd___device_maxd_version: device
								? t(", maxd {{value1}}", { value1: device.maxd_version })
								: "",
						})
					: maxdDetail;
		checks.push(check("maxd", "maxd", "maxd", maxdLocal, detail));
	} else {
		checks.push(check("ssh", "SSH", "SSH", sshState, sshDetail));
		checks.push(check("maxd", "maxd", "maxd", maxdState, maxdDetail));
	}

	const top = agents.filter((a) => a.parent === null);
	const down = top.filter((a) => a.link.state === "down").length;
	const asleep = top.filter((a) => a.link.state === "live" && a.asleep === true).length;
	const live = top.filter((a) => a.link.state === "live").length - asleep;
	let agentsState: CheckState = "unknown";
	let agentsDetail =
		top.length > 0
			? t("Unknown. {{plural_top_length___agent}} last seen here", {
					plural_top_length___agent: plural(top.length, "agent"),
				})
			: t("Unknown");
	if (state.state === "connected") {
		agentsState = "passed";
		agentsDetail =
			top.length === 0
				? t("None running")
				: down > 0 || asleep > 0
					? [
							t("{{value1}} running", { value1: live }),
							...(asleep > 0 ? [t("{{value1}} asleep", { value1: asleep })] : []),
							...(down > 0 ? [t("{{down}} not running", { down: down })] : []),
						].join(", ")
					: t("{{value1}} running", { value1: plural(live, "agent") });
	}
	checks.push(check("agents", t("Agent"), t("Agents"), agentsState, agentsDetail));

	// A check with no reading passed if any later one did.
	for (let i = checks.length - 1, later = false; i >= 0; i--) {
		const c = checks[i]!;
		if (c.state === "passed") later = true;
		else if (c.state === "unknown" && later) {
			checks[i] = { ...c, state: "passed", detail: IMPLIED[c.id] };
		}
	}
	// Nothing after a failure runs.
	const failed = checks.findIndex((c) => c.state === "failed");
	if (failed >= 0) {
		for (let i = failed + 1; i < checks.length; i++) {
			const c = checks[i]!;
			checks[i] = {
				...c,
				state: c.id === "agents" ? "unknown" : "not_run",
				detail: c.id === "agents" ? c.detail : t("Not run"),
			};
		}
	}
	return checks;
};

/** A jump host in front of the device that isn't connected, if any. */
const brokenJumpHost = (ctx: LinkContext): DeviceLink | undefined =>
	ctx.jumpHosts?.find((j) => j.state.state === "unreachable" || j.state.state === "connecting");

/**
 * The device's own dot and hover. A device behind a jump host that can't be
 * reached shares that host's problem.
 */
export const deviceConnection = (ctx: LinkContext, now: Timestamp): Connection => {
	const { link, device } = ctx;
	const held = heldConnection(link, ctx.claimed);
	if (held) return held;
	const name = link.name;
	const facts: Fact[] = [{ label: t("Route"), value: routeText(link) }];
	if (link.via.length > 0) facts.push({ label: t("Through"), value: link.via.join(", ") });
	const jump = brokenJumpHost(ctx);
	let look = lookOfLink(link);
	if (jump && look === "unknown")
		look = jump.state.state === "unreachable" ? "unreachable" : "connecting";
	const state = link.state;

	switch (look) {
		case "live": {
			if (link.peer?.path.type === "direct")
				facts.push({
					label: t("Path"),
					value: t("Direct, {{link_peer_path_address}}", {
						link_peer_path_address: link.peer.path.address,
					}),
				});
			if (link.peer?.path.type === "relayed")
				facts.push({
					label: t("Path"),
					value: t("Relayed through {{link_peer_path_region}}", {
						link_peer_path_region: link.peer.path.region,
					}),
				});
			if (state.state === "connected")
				facts.push({ label: t("Since"), value: formatWhen(state.since, now) });
			return {
				look,
				card: {
					title: t("Connected to {{name}}", { name: name }),
					body: device
						? t("maxd {{value1}} answers.", { value1: device.maxd_version })
						: t("maxd answers."),
					facts,
				},
			};
		}
		case "connecting": {
			const checks = linkChecks(ctx, now);
			if (state.state !== "connecting") {
				return {
					look,
					card: {
						title: t("Waiting for {{jump__name____name}}", {
							jump__name____name: jump?.name ?? name,
						}),
						body: t(
							"{{name}} is reached through {{jump__name_____a_jump_host}}, which is still connecting.",
							{
								name: name,
								jump__name_____a_jump_host: jump?.name ?? t("a jump host"),
							}
						),
						facts,
					},
				};
			}
			const title =
				state.step === "installing_maxd"
					? t("Installing maxd on {{name}}", { name: name })
					: state.step === "handshake"
						? t("Connecting to maxd on {{name}}", { name: name })
						: state.step === "waiting_for_reboot"
							? t("Waiting for {{name}} to restart", { name: name })
							: state.step === "waiting_for_approval"
								? t("Waiting for approval on {{name}}", { name: name })
								: state.attempt > 1
									? t("Reconnecting to {{name}}", { name: name })
									: t("Connecting to {{name}}", { name: name });
			const body =
				state.step === "waiting_for_reboot"
					? t("{{name}} is restarting. MaxHQ connects again once it's back.", {
							name: name,
						})
					: state.step === "waiting_for_approval"
						? t(
								"{{name}} doesn't know this app's key yet. Allow it from the card on another device where MaxHQ is open, and this one connects.",
								{ name: name }
							)
						: t("{{value1}} try.", { value1: ordinal(state.attempt) });
			return {
				look,
				card: {
					title,
					body,
					facts,
					checks,
					foot:
						state.step === "waiting_for_reboot"
							? undefined
							: t("Agents on {{name}} keep running meanwhile", { name: name }),
				},
			};
		}
		case "unreachable": {
			let body: string;
			if (state.state === "unreachable") {
				const seen = state.last_seen
					? t(
							" Last reachable at {{formatWhen_state_last_seen__now}}, {{formatAgo_state_last_seen__now}}.",
							{
								formatWhen_state_last_seen__now: formatWhen(state.last_seen, now),
								formatAgo_state_last_seen__now: formatAgo(state.last_seen, now),
							}
						)
					: t(" Never reached yet.");
				body = `${sentence(state.reason)}.${seen}`;
			} else {
				body = t(
					"{{name}} is reached through {{jump__name_____a_jump_host}}, which isn't reachable.",
					{ name: name, jump__name_____a_jump_host: jump?.name ?? t("a jump host") }
				);
			}
			if (jump && state.state === "unreachable") {
				body += t(" It's reached through {{jump_name}}, which isn't reachable either.", {
					jump_name: jump.name,
				});
			}
			return {
				look,
				card: {
					title: t("Can't reach {{name}}", { name: name }),
					body,
					facts,
					foot: t("MaxHQ keeps trying in the background"),
				},
			};
		}
		case "agent_down":
		case "unknown":
			return {
				look: "unknown",
				card: {
					title: t("Not checked yet"),
					body: t("MaxHQ hasn't reached {{name}} yet, which isn't the same as offline.", {
						name: name,
					}),
					facts,
				},
			};
	}
};

const agentLinkLook = (link: AgentLink): ConnectionLook =>
	link.state === "live" ? "live" : link.state === "down" ? "agent_down" : "unknown";

/** The agent's dot and hover, from its device's link and its own. */
export const agentConnection = (
	agent: Agent,
	ctx: LinkContext | undefined,
	now: Timestamp
): Connection => {
	if (!ctx) {
		return {
			look: "unknown",
			card: {
				title: t("Device not known yet"),
				body: t("MaxHQ hasn't matched this agent's workspace to a device it connects to."),
				facts: [],
			},
		};
	}
	const device = deviceConnection(ctx, now);
	if (device.look !== "live") return device;

	const name = ctx.link.name;
	const look = agentLinkLook(agent.link);
	const facts: Fact[] = [
		{ label: t("Device"), value: name },
		...device.card.facts.filter((f) => f.label !== "Since"),
	];
	if (agent.pid !== null) facts.push({ label: t("Process"), value: String(agent.pid) });
	switch (agent.link.state) {
		case "live":
			return agent.asleep
				? {
						look: "asleep",
						card: {
							title: t("Asleep"),
							body: t(
								"{{name}} stopped this agent's CLI after it sat idle. Your next message starts it again, with nothing lost.",
								{ name: name }
							),
							facts,
						},
					}
				: {
						look,
						card: {
							title: t("Live"),
							body: t("{{name}} answers and the agent runs.", { name: name }),
							facts,
						},
					};
		case "down": {
			const when = agent.link.since
				? t(" at {{value1}}", { value1: formatWhen(agent.link.since, now) })
				: "";
			return {
				look,
				card: {
					title: t("Agent not running"),
					body: t(
						"{{name}} answers, but the agent doesn't run. {{sentence_agent_link_reason}}{{when}}.",
						{
							name: name,
							sentence_agent_link_reason: sentence(agent.link.reason),
							when: when,
						}
					),
					facts,
				},
			};
		}
		case "unknown":
			return {
				look,
				card: {
					title: t("Agent state unknown"),
					body: t("{{name}} answers, but maxd can't tell yet whether the agent runs.", {
						name: name,
					}),
					facts,
				},
			};
	}
};
