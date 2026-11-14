// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's test hosts in its detail: the kinds it could host with their
// hosts, how long its leases last, and the leases it grants now. Leases
// stay on the device, so only agents there get one. Hidden when the
// device's `maxd` doesn't offer test hosts.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import { Picker } from "@/new-agent/picker";
import { ConfirmButton, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import type { DeviceId, LeaseId, TestHostSettings, TestLease } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import type { Option } from "@maxhq/shared/new-agent/spec";
import { leaseLine } from "@maxhq/shared/test-hosts/format";
import {
	backendLabel,
	kindLabel,
	leasesOn,
	offersTestHosts,
	testHostSettingsOf,
} from "@maxhq/shared/test-hosts/hosts";
import { placesOn } from "@maxhq/shared/test-hosts/place";
import type { ViewState } from "@maxhq/shared/view/replica";
import { actorName } from "@maxhq/shared/view/select";

import { LeaseView } from "./lease-view";
import { CheckAgain, PlaceBlock, useCall, useSaveTestHosts } from "./places";

/** Where a remote agent gets a lease, since leases stay on their device. */
export const localNote = (name: string): string =>
	t(
		"Only agents on {{name}} lease its test machines. For an agent elsewhere to test here, start an agent on {{name2}}.",
		{ name: name, name2: name }
	);

const minutes = (choices: readonly number[], kept: number): Option[] =>
	[...new Set([...choices, kept])]
		.sort((a, b) => a - b)
		.map((m) => ({ value: String(m), label: t("{{m}} min", { m: formatNumber(m) }) }));

type Limit = "default_minutes" | "longest_minutes" | "idle_minutes" | "max_vms";

const LIMITS: readonly {
	key: Limit;
	title: string;
	sub: string;
	choices: readonly number[];
}[] = [
	{
		key: "default_minutes",
		get title() {
			return t("Lease length");
		},
		get sub() {
			return t("How long a lease lasts when the agent doesn't say.");
		},
		choices: [15, 30, 60, 120],
	},
	{
		key: "longest_minutes",
		get title() {
			return t("Longest lease");
		},
		get sub() {
			return t("The most an agent may ask for.");
		},
		choices: [60, 120, 240],
	},
	{
		key: "idle_minutes",
		get title() {
			return t("Ends when idle");
		},
		get sub() {
			return t("A lease ends after this long without a command.");
		},
		choices: [5, 10, 20, 30],
	},
];

const Limits = ({
	device,
	view,
	live,
	mac,
}: {
	device: DeviceId;
	view: ViewState;
	live: boolean;
	mac: boolean;
}) => {
	const { t } = useI18n();

	const save = useSaveTestHosts(view);
	const { note, busy, call } = useCall();
	const kept = testHostSettingsOf(view, device);
	const change = (key: Limit, value: number) =>
		void call(t("Saving"), () =>
			save(device, (s): TestHostSettings => ({ ...s, [key]: value }))
		);
	return (
		<>
			{LIMITS.map((limit) => (
				<Row key={limit.key} title={limit.title} sub={limit.sub}>
					<Picker
						label={limit.title}
						value={String(kept[limit.key])}
						options={minutes(limit.choices, kept[limit.key])}
						onChange={(value) => change(limit.key, Number(value))}
						disabled={!live || busy}
					/>
				</Row>
			))}
			{mac ? (
				<Row
					title={t("macOS VMs at once")}
					sub={t("Apple lets a Mac run 2. Each takes its memory while it runs.")}
				>
					<Picker
						label={t("macOS VMs at once")}
						value={String(kept.max_vms)}
						options={[
							{ value: "1", label: "1" },
							{ value: "2", label: "2" },
						]}
						onChange={(value) => change("max_vms", Number(value))}
						disabled={!live || busy}
					/>
				</Row>
			) : null}
			{note ? (
				<div className="border-t border-sidebar-border px-4 py-2">
					<StatusNote note={note} />
				</div>
			) : null}
		</>
	);
};

const LeaseRow = ({
	lease,
	view,
	onOpen,
}: {
	lease: TestLease;
	view: ViewState;
	onOpen: () => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(30_000);
	const { note, busy, call } = useCall();
	const host = view.tables.test_host.get(lease.host);
	const title = t("{{value1}} for {{value2}}", {
		value1: kindLabel(lease.kind),
		value2: actorName(view, lease.holder),
	});
	return (
		<Row
			title={title}
			sub={[leaseLine(lease, now), host ? backendLabel(host.backend) : null]
				.filter(Boolean)
				.join(", ")}
		>
			<StatusNote note={note} />
			<button
				type="button"
				className={SECONDARY}
				aria-label={t("Open {{title}}", { title: title })}
				onClick={onOpen}
			>
				{t("Open")}
			</button>
			<ConfirmButton
				label={t("End")}
				question={t("Its holder loses the test machine.")}
				confirm={t("End lease")}
				disabled={busy}
				onConfirm={() =>
					void call(t("Ending"), () => source.endLease(lease.device, lease.id))
				}
			/>
		</Row>
	);
};

/** The leases a device grants now, each opening its view. */
export const DeviceLeases = ({
	device,
	name,
	view,
}: {
	device: DeviceId;
	name: string;
	view: ViewState;
}) => {
	const { t } = useI18n();

	const [open, setOpen] = useState<LeaseId | null>(null);
	const leases = leasesOn(view, device);
	return (
		<Section
			title={t("Leases")}
			label={t("Leases on {{name}}", { name: name })}
			hint={t("Test machines agents hold now")}
		>
			{leases.map((lease) => (
				<LeaseRow
					key={lease.id}
					lease={lease}
					view={view}
					onOpen={() => setOpen(lease.id)}
				/>
			))}
			<p className="border-t border-sidebar-border px-4 py-3 text-xs text-muted-foreground first:border-t-0">
				{leases.length === 0
					? t("No agent holds a test machine on {{name}}. ", { name: name })
					: ""}
				{localNote(name)}
			</p>
			<LeaseView lease={open} onClose={() => setOpen(null)} />
		</Section>
	);
};

/** A device's test hosts and leases, or nothing when its `maxd` doesn't offer them. */
export const DeviceTestHosts = ({
	device,
	name,
	live,
	view,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
	view: ViewState;
}) => {
	const { t } = useI18n();

	if (!offersTestHosts(view, device)) return null;
	const places = placesOn(view, device);
	const mac = view.tables.device.get(device)?.os === "macos";
	return (
		<>
			<Section
				title={t("Test hosts")}
				label={t("Test hosts on {{name}}", { name: name })}
				hint={<CheckAgain device={device} name={name} live={live} />}
			>
				{places.map((place) => (
					<PlaceBlock
						key={place.kind}
						place={place}
						title={kindLabel(place.kind)}
						view={view}
						live={live}
						check={false}
					/>
				))}
				<Limits device={device} view={view} live={live} mac={mac} />
			</Section>
			<DeviceLeases device={device} name={name} view={view} />
		</>
	);
};
