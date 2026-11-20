// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One device's place for one kind of test machine, as the Test hosts page
// and the device detail both show it: the tick that picks the device, a
// hint where it suits the kind best, and once ticked, each host its `maxd`
// detected or built, with the setup text, Build and Reset. The Mac's iOS
// place adds the switch for its own Simulator.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { ConfirmButton, SECONDARY, StatusNote } from "@/settings/controls";
import { deviceSettingsOf } from "@maxhq/shared/settings/device-settings";
import { failureOf, type Note } from "@/settings/result";
import { Badge } from "@/tasks/activity";
import type { DeviceId, TestHost, TestHostSettings } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { useNow } from "@maxhq/shared/lib/clock";
import { testHostsChange } from "@maxhq/shared/test-hosts/device";
import { HOST_STATE, hostActions, hostLine, vmLine } from "@maxhq/shared/test-hosts/format";
import { backendLabel, kindLabel } from "@maxhq/shared/test-hosts/hosts";
import { type Place, withKind } from "@maxhq/shared/test-hosts/place";
import type { ViewState } from "@maxhq/shared/view/replica";

/** A call's note and whether it's on its way, for one row's buttons. */
export const useCall = () => {
	const [note, setNote] = useState<Note | null>(null);
	const [busy, setBusy] = useState(false);
	const call = async (sending: string, action: () => Promise<ActionResult>) => {
		setBusy(true);
		setNote({ text: sending, error: false });
		try {
			const failure = failureOf(await action());
			setNote(failure ? { text: failure, error: true } : null);
			return failure === null;
		} catch (error) {
			console.warn(`Couldn't finish: ${sending}`, error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
			return false;
		} finally {
			setBusy(false);
		}
	};
	return { note, busy, call };
};

/** Saves a change to a device's test host settings, keeping the rest as it has them. */
export const useSaveTestHosts = (view: ViewState) => {
	const { source } = useServices();
	return (device: DeviceId, change: (settings: TestHostSettings) => TestHostSettings) =>
		source.setDeviceSettings(testHostsChange(view, device, change));
};

/** Check again, which runs detection on the device once more. */
export const CheckAgain = ({
	device,
	name,
	live,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { note, busy, call } = useCall();
	return (
		<>
			<StatusNote note={note} />
			<button
				type="button"
				className={SECONDARY}
				aria-label={t("Check {{name}} again", { name: name })}
				disabled={!live || busy}
				onClick={() => void call(t("Checking"), () => source.checkTestHosts(device))}
			>
				{t("Check again")}
			</button>
		</>
	);
};

const HostRow = ({ host, name, live }: { host: TestHost; name: string; live: boolean }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(60_000);
	const { note, busy, call } = useCall();
	const can = hostActions(host);
	const state = HOST_STATE[host.state];
	const label = t("{{value1}} on {{name}}", { value1: backendLabel(host.backend), name: name });
	const reset = () => void call(t("Resetting"), () => source.resetTestHost(host.device, host.id));
	return (
		<div
			role="group"
			aria-label={label}
			className="flex items-start gap-3 rounded-lg border border-sidebar-border bg-subpanel px-3 py-2"
		>
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<span className="flex items-center gap-2 text-[12.5px] text-foreground">
					{backendLabel(host.backend)}
					<Badge tone={state.tone}>{state.label}</Badge>
				</span>
				<span
					className={
						host.state === "failed"
							? "selectable text-xs text-status-error-text"
							: "selectable text-xs text-muted-foreground"
					}
				>
					{hostLine(host, now)}
				</span>
				{host.vm ? (
					<span className="selectable text-xs text-muted-foreground">
						{vmLine(host.vm)}
					</span>
				) : null}
			</div>
			<div className="flex shrink-0 items-center gap-2">
				<StatusNote note={note} />
				{can.build ? (
					<button
						type="button"
						className={SECONDARY}
						aria-label={t("Build {{label}}", { label: label })}
						disabled={!live || busy}
						onClick={() =>
							void call(t("Building"), () =>
								source.buildTestHost(host.device, host.id)
							)
						}
					>
						{t("Build")}
					</button>
				) : null}
				{can.reset && host.state === "leased" ? (
					<ConfirmButton
						label={t("Reset")}
						question={t("This ends its lease.")}
						confirm={t("Reset")}
						disabled={!live || busy}
						onConfirm={reset}
					/>
				) : can.reset ? (
					<button
						type="button"
						className={SECONDARY}
						aria-label={t("Reset {{label}}", { label: label })}
						disabled={!live || busy}
						onClick={reset}
					>
						{t("Reset")}
					</button>
				) : null}
			</div>
		</div>
	);
};

// The Mac's own Simulator, off by default, for when the VM isn't there.
const MacHostSwitch = ({ place, view, live }: { place: Place; view: ViewState; live: boolean }) => {
	const { t } = useI18n();

	const save = useSaveTestHosts(view);
	const { note, busy, call } = useCall();
	const on = deviceSettingsOf(view, place.device).test_hosts.mac_host ?? false;
	return (
		<div className="flex items-start gap-3 rounded-lg border border-sidebar-border bg-subpanel px-3 py-2">
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<span className="text-[12.5px] text-foreground">
					{t("Use this Mac's own Simulator")}
				</span>
				<span className="text-xs text-muted-foreground">
					{t(
						"For when the VM isn't there or runs too slowly. Apps it runs can read your home folder, so only agents on this Mac get it. Off by default."
					)}
				</span>
			</div>
			<div className="flex shrink-0 items-center gap-2">
				<StatusNote note={note} />
				<Switch
					aria-label={t("Use {{name}}'s own Simulator", { name: place.name })}
					checked={on}
					disabled={!live || busy}
					onCheckedChange={(next) =>
						void call(t("Saving"), () =>
							save(place.device, (s) => ({ ...s, mac_host: next }))
						)
					}
				/>
			</div>
		</div>
	);
};

/**
 * One place: the tick, then while ticked each host and its actions. `title`
 * names the row, the device on the Test hosts page and the kind on the
 * device's detail.
 */
export const PlaceBlock = ({
	place,
	title,
	view,
	live,
	check,
}: {
	place: Place;
	title: string;
	view: ViewState;
	live: boolean;
	/** Whether the row offers Check again for its device. */
	check: boolean;
}) => {
	const { t } = useI18n();

	const save = useSaveTestHosts(view);
	const { note, busy, call } = useCall();
	const mac = view.tables.device.get(place.device)?.os === "macos";
	return (
		<div className="flex flex-col gap-2 border-t border-sidebar-border px-4 py-3 first:border-t-0">
			<div className="flex items-center gap-3">
				<label className="flex min-w-0 flex-1 items-center gap-2 text-[12.5px] text-foreground">
					<Checkbox
						aria-label={t("Host {{value1}} on {{name}}", {
							value1: kindLabel(place.kind),
							name: place.name,
						})}
						checked={place.ticked}
						disabled={!live || busy}
						onCheckedChange={(on) =>
							void call(t("Saving"), () =>
								save(place.device, (s) => withKind(s, place.kind, on))
							)
						}
					/>
					<span className="truncate">{title}</span>
					{place.suggested ? (
						<span className="truncate text-[11.5px] text-muted-foreground">
							{place.suggested}
						</span>
					) : null}
				</label>
				<StatusNote note={note} />
				{check && place.ticked ? (
					<CheckAgain device={place.device} name={place.name} live={live} />
				) : null}
			</div>
			{!live ? (
				<p className="text-xs text-muted-foreground">
					{t("Connect to {{name}} to change it.", { name: place.name })}
				</p>
			) : null}
			{place.ticked ? (
				<div className="flex flex-col gap-1.5 pl-6">
					{place.hosts.length === 0 ? (
						<p className="text-xs text-muted-foreground">
							{t("Checking what {{name}} has.", { name: place.name })}
						</p>
					) : (
						place.hosts.map((host) => (
							<HostRow key={host.id} host={host} name={place.name} live={live} />
						))
					)}
					{place.kind === "ios_simulator" && mac ? (
						<MacHostSwitch place={place} view={view} live={live} />
					) : null}
				</div>
			) : null}
		</div>
	);
};
