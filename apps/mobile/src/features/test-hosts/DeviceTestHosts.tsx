// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's test hosts on its page, as the desktop shows them: Check
// again, each kind it could host with its switch and, once on, each host
// with Build and Reset, how long leases last, and the leases it grants now,
// each opening its own screen. Changing which kinds it hosts, the limits
// and the Mac's own Simulator saves the device's settings, and Build
// installs software while Reset wipes the machine, so the Mac lets this
// phone do those only with its switch on. Check again stays open. Hidden
// when the device's `maxd` doesn't offer test hosts.

import { useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { DeviceId, TestHost, TestHostSettings, TestLease } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { deviceSettingsOf } from "@maxhq/shared/settings/device-settings";
import {
	LEASE_LIMITS,
	type LeaseLimit,
	leaseSub,
	leaseTitle,
	localNote,
	minuteChoices,
	testHostsChange,
	VM_CHOICES,
} from "@maxhq/shared/test-hosts/device";
import { HOST_STATE, hostActions, hostLine, vmLine } from "@maxhq/shared/test-hosts/format";
import {
	backendLabel,
	kindLabel,
	leasesOn,
	offersTestHosts,
	testHostSettingsOf,
} from "@maxhq/shared/test-hosts/hosts";
import { type Place, placesOn, withKind } from "@maxhq/shared/test-hosts/place";
import type { ViewState } from "@maxhq/shared/view/replica";

import type { Screens } from "../../navigation";
import { useServices } from "../../services";
import { color, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { choose, confirm } from "../../ui/choose";
import { Chevron } from "../../ui/Icon";
import {
	Buttons,
	CardBody,
	Muted,
	NoteLine,
	type SectionProps,
	useAction,
} from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { Badge, GatedToggle } from "./parts";

/** Saves a change to a device's test host settings, keeping the rest as it has them. */
const useSave = (view: ViewState) => {
	const { source } = useServices();
	return (device: DeviceId, change: (settings: TestHostSettings) => TestHostSettings) =>
		source.setDeviceSettings(testHostsChange(view, device, change));
};

const CheckAgain = ({ device, name, live }: { device: DeviceId; name: string; live: boolean }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	return (
		<CardBody>
			{live ? null : <Muted>{t("Connect to {{name}} to change it.", { name: name })}</Muted>}
			<Buttons>
				<Button
					small
					label={t("Check again")}
					hint={t("Check {{name}} again", { name: name })}
					disabled={!live || busy}
					onPress={() => void run(t("Checking"), () => source.checkTestHosts(device))}
				/>
			</Buttons>
			<NoteLine note={note} />
		</CardBody>
	);
};

const HostBlock = ({
	host,
	name,
	live,
	lets,
}: {
	host: TestHost;
	name: string;
	live: boolean;
	lets: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(60_000);
	const { busy, note, run } = useAction();
	const can = hostActions(host);
	const off = !live || !lets || busy;
	const state = HOST_STATE[host.state];
	const label = t("{{value1}} on {{name}}", { value1: backendLabel(host.backend), name: name });
	const reset = () => void run(t("Resetting"), () => source.resetTestHost(host.device, host.id));
	return (
		<View accessibilityLabel={label} style={styles.host}>
			<View style={styles.hostTop}>
				<Text style={styles.hostName} numberOfLines={1}>
					{backendLabel(host.backend)}
				</Text>
				<Badge label={state.label} tone={state.tone} />
			</View>
			<Text style={host.state === "failed" ? styles.failed : styles.line}>
				{hostLine(host, now)}
			</Text>
			{host.vm ? <Text style={styles.line}>{vmLine(host.vm)}</Text> : null}
			{can.build || can.reset ? (
				<Buttons>
					{can.build ? (
						<Button
							small
							label={t("Build")}
							hint={t("Build {{label}}", { label: label })}
							disabled={off}
							onPress={() =>
								void run(t("Building"), () =>
									source.buildTestHost(host.device, host.id)
								)
							}
						/>
					) : null}
					{can.reset ? (
						<Button
							small
							label={t("Reset")}
							hint={t("Reset {{label}}", { label: label })}
							disabled={off}
							onPress={() =>
								host.state === "leased"
									? confirm(
											{
												title: t("Reset {{label}}", { label: label }),
												detail: t("This ends its lease."),
											},
											t("Reset"),
											reset
										)
									: reset()
							}
						/>
					) : null}
				</Buttons>
			) : null}
			<NoteLine note={note} />
		</View>
	);
};

// The Mac's own Simulator, off by default, for when the VM isn't there.
const MacHostSwitch = ({ place, view, off }: { place: Place; view: ViewState; off: boolean }) => {
	const { t } = useI18n();

	const save = useSave(view);
	const { busy, note, run } = useAction();
	const on = deviceSettingsOf(view, place.device).test_hosts.mac_host ?? false;
	const label = t("Use {{name}}'s own Simulator", { name: place.name });
	return (
		<View style={styles.host}>
			<View style={styles.hostTop}>
				<Text style={[styles.hostName, styles.grow]}>{label}</Text>
				<GatedToggle
					label={label}
					value={on}
					disabled={off || busy}
					onChange={(next) =>
						void run(t("Saving"), () =>
							save(place.device, (s) => ({ ...s, mac_host: next }))
						)
					}
				/>
			</View>
			<Text style={styles.line}>
				{t(
					"For when the VM isn't there or runs too slowly. Apps it runs can read your home folder, so only agents on this Mac get it. Off by default."
				)}
			</Text>
			<NoteLine note={note} />
		</View>
	);
};

/** One kind the device could host: its switch, then while on each host. */
const PlaceRows = ({
	place,
	view,
	live,
	lets,
}: {
	place: Place;
	view: ViewState;
	live: boolean;
	lets: boolean;
}) => {
	const { t } = useI18n();

	const save = useSave(view);
	const { busy, note, run } = useAction();
	const mac = view.tables.device.get(place.device)?.os === "macos";
	const off = !live || !lets;
	const ios = place.kind === "ios_simulator" && mac;
	return (
		<>
			<CardRow label={kindLabel(place.kind)} note={place.suggested ?? undefined}>
				<GatedToggle
					label={t("Host {{value1}} on {{name}}", {
						value1: kindLabel(place.kind),
						name: place.name,
					})}
					value={place.ticked}
					disabled={off || busy}
					onChange={(on) =>
						void run(t("Saving"), () =>
							save(place.device, (s) => withKind(s, place.kind, on))
						)
					}
				/>
			</CardRow>
			{note || place.ticked ? (
				<View style={styles.under}>
					<NoteLine note={note} />
					{place.ticked && place.hosts.length === 0 ? (
						<Muted>{t("Checking what {{name}} has.", { name: place.name })}</Muted>
					) : null}
					{place.ticked
						? place.hosts.map((host) => (
								<HostBlock
									key={host.id}
									host={host}
									name={place.name}
									live={live}
									lets={lets}
								/>
							))
						: null}
					{place.ticked && ios ? (
						<MacHostSwitch place={place} view={view} off={off} />
					) : null}
				</View>
			) : null}
		</>
	);
};

type Limit = LeaseLimit | "max_vms";

/** A limit's row, which picks from its choices in a sheet. */
const LimitRow = ({
	title,
	sub,
	options,
	kept,
	off,
	onPick,
}: {
	title: string;
	sub: string;
	options: readonly { readonly value: number; readonly label: string }[];
	kept: number;
	off: boolean;
	onPick: (value: number) => void;
}) => {
	const shown = options.find((o) => o.value === kept)?.label ?? String(kept);
	const pick = () =>
		choose(
			title,
			options.map((o) => ({
				label: o.label,
				disabled: o.value === kept,
				pick: () => onPick(o.value),
			})),
			sub
		);
	return (
		<CardRow label={title} note={sub} onPress={off ? undefined : pick}>
			<Text style={[styles.value, off && styles.dim]}>{shown}</Text>
		</CardRow>
	);
};

const Limits = ({
	device,
	view,
	live,
	mac,
	lets,
}: {
	device: DeviceId;
	view: ViewState;
	live: boolean;
	mac: boolean;
	lets: boolean;
}) => {
	const { t } = useI18n();

	const save = useSave(view);
	const { busy, note, run } = useAction();
	const kept = testHostSettingsOf(view, device);
	const off = !live || !lets || busy;
	const change = (key: Limit, value: number) =>
		void run(t("Saving"), () =>
			save(device, (s): TestHostSettings => ({ ...s, [key]: value }))
		);
	return (
		<>
			{LEASE_LIMITS.map((limit) => (
				<LimitRow
					key={limit.key}
					title={limit.title}
					sub={limit.sub}
					options={minuteChoices(limit.choices, kept[limit.key])}
					kept={kept[limit.key]}
					off={off}
					onPick={(value) => change(limit.key, value)}
				/>
			))}
			{mac ? (
				<LimitRow
					title={t("macOS VMs at once")}
					sub={t("Apple lets a Mac run 2. Each takes its memory while it runs.")}
					options={VM_CHOICES.map((n) => ({ value: n, label: String(n) }))}
					kept={kept.max_vms}
					off={off}
					onPick={(value) => change("max_vms", value)}
				/>
			) : null}
			{note ? (
				<CardBody>
					<NoteLine note={note} />
				</CardBody>
			) : null}
		</>
	);
};

const LeaseRow = ({ lease, view }: { lease: TestLease; view: ViewState }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const now = useNow(30_000);
	const { busy, note, run } = useAction();
	const title = leaseTitle(view, lease);
	const end = () =>
		confirm(
			{ title: t("End lease"), detail: t("Its holder loses the test machine.") },
			t("End lease"),
			() => void run(t("Ending"), () => source.endLease(lease.device, lease.id))
		);
	return (
		<View style={styles.lease}>
			<Pressable
				accessibilityRole="link"
				accessibilityLabel={t("Open {{title}}", { title: title })}
				accessibilityHint={leaseSub(view, lease, now)}
				onPress={() =>
					navigation.navigate("Lease", { device: lease.device, lease: lease.id })
				}
				style={({ pressed }) => [styles.leaseOpen, pressed && styles.pressed]}
			>
				<View style={styles.grow}>
					<Text style={styles.leaseTitle}>{title}</Text>
					<Text style={styles.line}>{leaseSub(view, lease, now)}</Text>
					<NoteLine note={note} />
				</View>
				<Chevron dir="right" size={12} />
			</Pressable>
			<View style={styles.leaseEnd}>
				<Button small tone="danger" label={t("End")} disabled={busy} onPress={end} />
			</View>
		</View>
	);
};

/** The leases a device grants now, each opening its own screen. */
const DeviceLeases = ({
	device,
	name,
	view,
}: {
	device: DeviceId;
	name: string;
	view: ViewState;
}) => {
	const { t } = useI18n();

	const leases = leasesOn(view, device);
	return (
		<Card title={t("Leases")} note={t("Test machines agents hold now")}>
			{leases.map((lease) => (
				<LeaseRow key={lease.id} lease={lease} view={view} />
			))}
			<CardBody>
				<Muted>
					{leases.length === 0
						? t("No agent holds a test machine on {{name}}. ", { name: name })
						: ""}
					{localNote(name)}
				</Muted>
			</CardBody>
		</Card>
	);
};

/** A device's test hosts and leases, or nothing when its `maxd` doesn't offer them. */
export const DeviceTestHosts = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();

	const lets = useLetsPhone(device);
	if (!offersTestHosts(view, device)) return null;
	const places = placesOn(view, device);
	const mac = view.tables.device.get(device)?.os === "macos";
	return (
		<>
			<Card title={t("Test hosts")}>
				<CheckAgain device={device} name={name} live={live} />
				{places.map((place) => (
					<PlaceRows key={place.kind} place={place} view={view} live={live} lets={lets} />
				))}
				<Limits device={device} view={view} live={live} mac={mac} lets={lets} />
				{lets ? null : (
					<CardBody>
						<NeedsSwitch />
					</CardBody>
				)}
			</Card>
			<DeviceLeases device={device} name={name} view={view} />
		</>
	);
};

const styles = StyleSheet.create({
	under: { paddingHorizontal: 14, paddingBottom: 12, gap: 8 },
	host: {
		gap: 4,
		padding: 10,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	hostTop: { flexDirection: "row", alignItems: "center", gap: 8 },
	hostName: { flexShrink: 1, color: color.foreground, fontSize: size.small },
	grow: { flex: 1 },
	line: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText, fontSize: size.meta },
	value: { color: color.secondaryForeground, fontSize: size.small },
	dim: { opacity: 0.5 },
	lease: {
		flexDirection: "row",
		alignItems: "center",
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	leaseOpen: {
		flex: 1,
		minHeight: 44,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingLeft: 14,
		paddingRight: 8,
		paddingVertical: 10,
	},
	leaseTitle: { color: color.foreground, fontSize: size.small },
	leaseEnd: { paddingRight: 14 },
	pressed: { backgroundColor: color.accent },
});
