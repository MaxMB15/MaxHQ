// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The cards above the composer for the ports the agent's processes listen
// on, as the desktop shows them. A port offers a forward to this phone, and
// once forwarded the card says where it landed, with Open and Stop
// forwarding. A device that doesn't forward ports gets no offer, and neither
// does one that can't be reached. Its forwards stay, saying so. When a port
// this phone forwarded closes, a quiet line says the forward stopped.
// Forward needs Let this phone do everything on the Mac, and one line says
// so while it's off. The cards only follow the whole view while the agent
// has a port open or a notice to show.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import Plug from "lucide-react-native/icons/plug";
import Unplug from "lucide-react-native/icons/unplug";
import X from "lucide-react-native/icons/x";
import { type ReactNode, useEffect, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { Agent, ListeningPort } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { type PortActions, usePortActions } from "@maxhq/shared/ports/actions";
import {
	agentPorts,
	type ClosedForward,
	forwardsPorts,
	isRemoteDevice,
	isUnreachable,
	subjectName,
} from "@maxhq/shared/ports/select";
import { dismissClosed, dismissPort, usePorts, watchClosed } from "@maxhq/shared/ports/store";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices, useView } from "../../services";
import { color, font, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Spinner } from "../../ui/Dot";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { PortPage } from "./PortPage";
import {
	dropForward,
	myNotices,
	noticeForwards,
	offerCards,
	openPortCount,
	PHONE_HOOKS,
	type PhoneForward,
	useMyForwards,
	watchMyForwards,
} from "./running";

const plain = (n: number) => formatNumber(n, { useGrouping: false });

/** Where a card's port is, such as "MaxHQ" on "devbox". */
type Place = { readonly subject: string; readonly device: string };

/** A small square button with an icon, its hit area grown to a finger's. */
const IconAction = ({
	label,
	disabled,
	onPress,
	children,
}: {
	label: string;
	disabled?: boolean;
	onPress: () => void;
	children: ReactNode;
}) => (
	<Pressable
		accessibilityRole="button"
		accessibilityLabel={label}
		accessibilityState={{ disabled: Boolean(disabled) }}
		disabled={disabled}
		hitSlop={6}
		onPress={onPress}
		style={({ pressed }) => [styles.icon, pressed && styles.pressed, disabled && styles.off]}
	>
		{children}
	</Pressable>
);

/** One port's card: a plug, what it says, and its buttons. */
const Card = ({
	forwarded,
	title,
	detail,
	failure,
	children,
}: {
	forwarded: boolean;
	title: string;
	detail: ReactNode;
	failure: string | null;
	children: ReactNode;
}) => (
	<View style={styles.card}>
		<View style={[styles.plug, forwarded ? styles.plugForwarded : styles.plugOffered]}>
			<Plug size={13} strokeWidth={2.2} color={forwarded ? color.readyText : color.primary} />
		</View>
		<View style={styles.text}>
			<Text numberOfLines={2} style={styles.title}>
				{title}
			</Text>
			<Text numberOfLines={1} style={styles.detail}>
				{detail}
			</Text>
			{failure !== null ? (
				<Text accessibilityRole="alert" style={styles.failure}>
					{failure}
				</Text>
			) : null}
		</View>
		<View style={styles.buttons}>{children}</View>
	</View>
);

const OfferCard = ({
	port,
	place,
	lets,
	actions,
}: {
	port: ListeningPort;
	place: Place;
	/** Whether the device takes this phone's forwards. */
	lets: boolean;
	actions: PortActions;
}) => {
	const { t } = useI18n();

	const busy = actions.busy(port.id);
	return (
		<Card
			forwarded={false}
			title={t("{{subject}} on {{device}} is listening on port {{port}}.", {
				subject: place.subject,
				device: place.device,
				port: plain(port.port),
			})}
			detail={
				<>
					{t("Forward to ")}
					<Text style={styles.code}>{`localhost:${plain(port.port)}`}</Text>?
				</>
			}
			failure={actions.failure(port.id)}
		>
			<Button
				label={t("Forward")}
				hint={t("Forward port {{port}}", { port: plain(port.port) })}
				tone="primary"
				small
				disabled={busy || !lets}
				icon={busy ? <Spinner size={10} tone={color.primaryForeground} /> : undefined}
				onPress={() => actions.forward(port)}
			/>
			<IconAction label={t("Dismiss")} onPress={() => dismissPort(port.id)}>
				<X size={14} strokeWidth={2.2} color={color.secondaryForeground} />
			</IconAction>
		</Card>
	);
};

const ForwardedCard = ({
	port,
	forward,
	place,
	offline,
	actions,
	onOpen,
}: {
	port: ListeningPort;
	forward: PhoneForward;
	place: Place;
	/** Whether the device can't be reached right now. */
	offline: boolean;
	actions: PortActions;
	onOpen: (local: number) => void;
}) => {
	const { t } = useI18n();

	const id = forward.forward;
	const stopping = actions.busy(port.id);
	const moved = forward.local !== port.port;
	const from = t("{{subject}} on {{device}}", { subject: place.subject, device: place.device });
	return (
		<Card
			forwarded
			title={
				moved
					? t("{{port}} is busy on this phone, forwarded to {{local_port}}", {
							port: plain(port.port),
							local_port: plain(forward.local),
						})
					: t("Forwarded to localhost:{{local_port}}", {
							local_port: plain(forward.local),
						})
			}
			detail={
				offline
					? t("{{device}} can't be reached. The forward picks up again once it's back.", {
							device: place.device,
						})
					: moved
						? t("Something on this phone already uses {{port}}.", {
								port: plain(port.port),
							})
						: port.process
							? t("{{name}} from {{from}}", { name: port.process.name, from })
							: t("From {{from}}", { from })
			}
			failure={actions.failure(port.id)}
		>
			<Button label={t("Open")} small onPress={() => onOpen(forward.local)} />
			<IconAction
				label={t("Stop forwarding")}
				disabled={stopping || id === null}
				onPress={() => {
					if (id === null) return;
					// Once stopped, the offer stays away. Ports and processes still lists the port.
					dismissPort(port.id);
					actions.stop(port, id);
				}}
			>
				{stopping ? (
					<Spinner size={11} tone={color.mutedForeground} />
				) : (
					<Unplug size={14} strokeWidth={2.1} color={color.secondaryForeground} />
				)}
			</IconAction>
		</Card>
	);
};

const ClosedLine = ({ notice, device }: { notice: ClosedForward; device: string }) => {
	const { t } = useI18n();

	const dismiss = () => {
		dismissClosed(notice.agent, notice.port);
		for (const port of noticeForwards(notice, useMyForwards.getState().mine)) dropForward(port);
	};
	return (
		<View style={styles.closed}>
			<Plug size={12} strokeWidth={2.2} color={color.mutedForeground} />
			<Text numberOfLines={2} style={styles.closedText}>
				{t(
					"Port {{port}} closed on {{device}}. Forwarding to localhost:{{localPort}} stopped.",
					{
						port: plain(notice.port),
						device,
						localPort: plain(notice.localPort),
					}
				)}
			</Text>
			<IconAction
				label={t("Dismiss the notice about port {{port}}", { port: plain(notice.port) })}
				onPress={dismiss}
			>
				<X size={13} strokeWidth={2.4} color={color.mutedForeground} />
			</IconAction>
		</View>
	);
};

/** The cards themselves, following the whole view. */
const Cards = ({ agent }: { agent: Agent }) => {
	const { source } = useServices();
	const view = useView((s) => s.view);
	const dismissed = usePorts((s) => s.dismissed);
	const closed = usePorts((s) => s.closed);
	const mine = useMyForwards((s) => s.mine);
	const actions = usePortActions(source, PHONE_HOOKS);
	const now = useNow(60_000);
	const [page, setPage] = useState<number | null>(null);

	const workspace = view.tables.workspace.get(agent.workspace);
	const device = workspace?.device ?? null;
	const lets = useLetsPhone(device);
	if (!workspace || device === null) return null;

	const remote = isRemoteDevice(view, device);
	const forwards = forwardsPorts(view, device);
	const offline = remote && isUnreachable(view, device, now);
	const place = { subject: subjectName(view, workspace), device: deviceName(view, device) };
	// Every device is another computer to the phone, so each port gets a forward to offer.
	const cards = offerCards(view, agent.id, mine, dismissed, forwards && !offline);
	const notices = myNotices(closed, mine, agent.id, agentPorts(view, agent.id));
	const offered = cards.some((card) => card.forward === null);
	if (cards.length === 0 && notices.length === 0 && page === null) return null;

	return (
		<View style={styles.list}>
			{cards.map(({ port, forward }) =>
				forward ? (
					<ForwardedCard
						key={port.id}
						port={port}
						forward={forward}
						place={place}
						offline={offline}
						actions={actions}
						onOpen={setPage}
					/>
				) : (
					<OfferCard
						key={port.id}
						port={port}
						place={place}
						lets={lets}
						actions={actions}
					/>
				)
			)}
			{offered && !lets ? <NeedsSwitch /> : null}
			{notices.map((notice) => (
				<ClosedLine
					key={`closed-${notice.port}`}
					notice={notice}
					device={deviceName(view, notice.device)}
				/>
			))}
			<PortPage local={page} onClose={() => setPage(null)} />
		</View>
	);
};

/** The agent's ports above its composer, or nothing while it listens on none. */
export const PortOffers = ({ agent }: { agent: Agent }) => {
	const { view: store } = useServices();
	useEffect(() => watchClosed(store), [store]);
	useEffect(() => watchMyForwards(store), [store]);

	// A cheap look on every change, so the cards follow the view only while they show.
	const open = useView((s) => openPortCount(s.view, agent.id) > 0);
	const notice = usePorts((s) => s.closed.some((closed) => closed.agent === agent.id));
	return open || notice ? <Cards agent={agent} /> : null;
};

const styles = StyleSheet.create({
	list: { gap: 6, paddingHorizontal: 12, paddingBottom: 6 },
	card: {
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		minHeight: 48,
		paddingVertical: 7,
		paddingLeft: 10,
		paddingRight: 8,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.card,
	},
	plug: {
		width: 26,
		height: 26,
		borderRadius: radius.sm,
		alignItems: "center",
		justifyContent: "center",
	},
	plugOffered: { backgroundColor: tint(color.primary, 0.14) },
	plugForwarded: { backgroundColor: tint(color.ready, 0.14) },
	text: { flex: 1, minWidth: 0, gap: 2 },
	title: { color: color.foreground, fontSize: size.meta, lineHeight: 16 },
	detail: { color: color.mutedForeground, fontSize: size.pill },
	code: { color: color.secondaryForeground, fontFamily: font.mono, fontSize: size.pill },
	failure: { color: color.errorText, fontSize: size.pill },
	buttons: { flexDirection: "row", alignItems: "center", gap: 4 },
	icon: {
		width: 32,
		height: 32,
		borderRadius: radius.sm,
		alignItems: "center",
		justifyContent: "center",
	},
	pressed: { backgroundColor: color.accent },
	off: { opacity: 0.4 },
	closed: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		minHeight: 36,
		paddingLeft: 10,
		paddingRight: 2,
		borderRadius: radius.lg,
		borderWidth: 1,
		borderStyle: "dashed",
		borderColor: color.input,
	},
	closedText: { flex: 1, minWidth: 0, color: color.mutedForeground, fontSize: size.pill },
});
