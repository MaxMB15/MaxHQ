// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's test machines: in its Settings tab the kinds it could host
// with their hosts and how long its leases last, and in its detail the
// leases it grants now. Leases stay on the device, so only agents there
// get one. The limits and the lease's words are in shared
// `test-hosts/device`, which the phone shows too.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import { Picker } from "@/new-agent/picker";
import { ConfirmButton, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { deviceCardId } from "@/settings/store";
import type { DeviceId, LeaseId, TestHostSettings, TestLease } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	LEASE_LIMITS,
	type LeaseLimit,
	leaseSub,
	leaseTitle,
	localNote,
	minuteChoices,
	VM_CHOICES,
} from "@maxhq/shared/test-hosts/device";
import { kindLabel, leasesOn, testHostSettingsOf } from "@maxhq/shared/test-hosts/hosts";
import { placesOn } from "@maxhq/shared/test-hosts/place";
import type { ViewState } from "@maxhq/shared/view/replica";

import { LeaseView } from "./lease-view";
import { CheckAgain, PlaceBlock, useCall, useSaveTestHosts } from "./places";

type Limit = LeaseLimit | "max_vms";

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
			{LEASE_LIMITS.map((limit) => (
				<Row key={limit.key} title={limit.title} sub={limit.sub}>
					<Picker
						label={limit.title}
						value={String(kept[limit.key])}
						options={minuteChoices(limit.choices, kept[limit.key]).map((m) => ({
							value: String(m.value),
							label: m.label,
						}))}
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
						options={VM_CHOICES.map((n) => ({ value: String(n), label: String(n) }))}
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
	const title = leaseTitle(view, lease);
	return (
		<Row title={title} sub={leaseSub(view, lease, now)}>
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

/**
 * A device's test machines in its Settings tab, which was its part of the
 * Test hosts page: each kind it could host with its hosts, how long its
 * leases last, and Check again. Its tab shows it only when the device's
 * `maxd` offers test hosts.
 */
export const TestMachines = ({
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

	const mac = view.tables.device.get(device)?.os === "macos";
	return (
		<Section
			title={t("Test machines")}
			id={deviceCardId("test-machines")}
			label={t("Test machines on {{name}}", { name: name })}
			hint={t("Simulators and VMs that agents here can lease")}
		>
			{placesOn(view, device).map((place) => (
				<PlaceBlock
					key={place.kind}
					place={place}
					title={kindLabel(place.kind)}
					view={view}
					live={live}
				/>
			))}
			<Limits device={device} view={view} live={live} mac={mac} />
			<div className="flex items-center gap-3 border-t border-sidebar-border px-4 py-2.5">
				<p className="min-w-0 flex-1 text-xs text-muted-foreground">
					{t(
						"Only agents on a device lease its test machines. For an agent elsewhere to test on one, start an agent on the device that hosts it."
					)}
				</p>
				<CheckAgain device={device} name={name} live={live} />
			</div>
		</Section>
	);
};
