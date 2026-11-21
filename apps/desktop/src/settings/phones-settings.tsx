// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Phones page: Add phone ticks the devices the phone may reach, picks
// the one it pairs through and shows the code to scan, and the list below
// has each paired phone with the devices that let it in, a Remove, a New
// code for the devices it hasn't signed in to over the network yet, and the
// switch that lets it do everything, which asks first. Once a phone pairs,
// a device without the APNs key says so, with the way to add it. A
// device's own settings take the address the phone dials it at.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Fragment, type KeyboardEvent, useMemo, useState } from "react";

import { useServices, useView } from "@/app/services";
import { ConsentCard } from "@/components/consent-card";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import type { DeviceId, PhoneDevice, PhoneId, PhonePairing } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { awaitingSignIn, CODE_MINUTES } from "@maxhq/shared/settings/network-access";
import {
	type PairedPhone,
	pairedPhones,
	pairingDevice,
	reachable,
	withoutPushKey,
} from "@maxhq/shared/settings/phones";
import { formatDuration } from "@maxhq/shared/status/format";

import { ConfirmButton, FIELD, PRIMARY, Row, SECONDARY, Section, StatusNote } from "./controls";
import { DeviceTicks, useTicks } from "./login-parts";
import { failureOf, type Note } from "./result";
import { Pick } from "./snooze-presets";
import { showPlace } from "./store";
import { formatList, t as translate, useI18n } from "@maxhq/shared/i18n";

const PHONE_DEVICES = "phone-devices";

const usePhoneDevices = () => {
	const { source } = useServices();
	return useQuery({ queryKey: [PHONE_DEVICES], queryFn: () => source.listPhoneDevices() });
};

/** The code to scan, dark modules on white with a quiet zone round it. */
const Code = ({ pairing }: { pairing: PhonePairing }) => {
	useI18n();
	const { size, path } = pairing.qr;
	return (
		<svg
			role="img"
			aria-label={translate("Pairing code")}
			viewBox={`-2 -2 ${size + 4} ${size + 4}`}
			shapeRendering="crispEdges"
			className="size-[208px] rounded-[9px] bg-white"
		>
			<path d={path} fill="#000" />
		</svg>
	);
};

const AddPhone = ({ devices }: { devices: readonly PhoneDevice[] }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow();
	const targets = useMemo(
		() => devices.map((d) => ({ device: d.device, name: d.name, live: d.note === null })),
		[devices]
	);
	const [ticked, setTicked] = useTicks(targets);
	const [chosen, setChosen] = useState<DeviceId | null>(null);
	const [pairing, setPairing] = useState<{ code: PhonePairing; from: number } | null>(null);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);

	const tickedList = reachable(devices).filter((d) => ticked.has(d));
	const through =
		chosen !== null && ticked.has(chosen) ? chosen : pairingDevice(view, tickedList);
	const paired = pairing
		? pairedPhones(view).find(
				(p) =>
					p.pairedAt >= pairing.from &&
					p.devices.some((d) => d.device === pairing.code.device)
			)
		: undefined;
	const expired = pairing !== null && !paired && now >= pairing.code.expires_at;

	const show = async () => {
		if (through === null) return;
		setBusy(true);
		setNote(null);
		setPairing(null);
		// The clock this render read, a second at most behind.
		const from = now;
		try {
			const result = await source.beginPhonePair(
				through,
				tickedList.filter((d) => d !== through)
			);
			if (result.status === "ok") setPairing({ code: result.data, from });
			else
				setNote({
					text: failureOf(result) ?? translate("Couldn't start pairing."),
					error: true,
				});
		} catch (error) {
			console.warn(translate("Couldn't start pairing"), error);
			setNote({ text: translate("Couldn't start pairing. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	const name = (device: DeviceId) => devices.find((d) => d.device === device)?.name ?? device;
	const unreachable = devices.filter((d) => d.note !== null);

	return (
		<Section
			title={translate("Add phone")}
			hint={translate("Scan the code with MaxHQ on the phone")}
		>
			<div className="flex flex-col gap-3 px-4 py-3">
				<DeviceTicks
					targets={targets}
					ticked={ticked}
					onChange={setTicked}
					disabled={busy}
					label={translate("Devices the phone can reach")}
					empty={translate("No device takes phones yet.")}
					// The line below says why the phone can't reach one.
					away={null}
				/>
				{unreachable.map((d) => (
					<span key={d.device} className="selectable text-xs text-muted-foreground">
						{d.name}: {d.note}
					</span>
				))}
				{tickedList.length > 1 && through !== null ? (
					<span className="flex items-center gap-2 text-[12.5px] text-foreground">
						{translate("Pair through")}
						<Pick
							label={translate("Pair through")}
							value={through}
							options={tickedList.map((d) => ({ value: d, label: name(d) }))}
							onChange={setChosen}
						/>
					</span>
				) : null}
				<span className="flex items-center gap-2.5">
					<button
						type="button"
						className={PRIMARY}
						disabled={busy || through === null}
						onClick={() => void show()}
					>
						{pairing ? translate("Show a new code") : translate("Show the code")}
					</button>
					<StatusNote note={note} />
				</span>
				{paired ? (
					<div className="flex flex-col gap-2">
						<span role="status" className="selectable text-[12.5px] text-foreground">
							{paired.name} {translate(" is paired.")}
						</span>
						<NoPushKey names={withoutPushKey(view, paired)} />
					</div>
				) : pairing && !expired ? (
					<div className="flex items-end gap-4">
						<Code pairing={pairing.code} />
						<span className="text-xs text-muted-foreground">
							{translate(
								"On the phone, choose Scan the code. It works once, for {{duration}} more.",
								{ duration: formatDuration(pairing.code.expires_at - now) }
							)}
						</span>
					</div>
				) : expired ? (
					<span role="status" className="text-xs text-muted-foreground">
						{translate("The code ran out. Show a new one.")}
					</span>
				) : null}
			</div>
		</Section>
	);
};

/**
 * The devices a new phone paired with that hold no APNs key, which can't
 * push to it until they get one, with the way to the key's card.
 */
const NoPushKey = ({ names }: { names: readonly string[] }) => {
	useI18n();
	const services = useServices();
	if (names.length === 0) return null;
	return (
		<span className="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
			{translate("{{devices}} has no APNs key yet, so it can't push to the phone.", {
				devices: formatList(names),
				count: names.length,
			})}
			<button
				type="button"
				className={SECONDARY}
				onClick={() => showPlace(services, { kind: "phones", card: "apns" })}
			>
				{translate("Add the key file")}
			</button>
		</span>
	);
};

/** What a phone that may do everything could do, for the card that asks first. */
const EVERYTHING_RISK =
	"Anyone holding this phone could start agents or set them to Auto or Full access, run commands and open shells on your devices, change settings, store or remove sign-ins, and copy files out. Adding keys or other phones still needs this Mac.";

/** The devices a phone's switch didn't reach, and when it will. */
const unreached = (names: readonly string[], everything: boolean) => {
	const list = names.join(", ");
	const they = names.length === 1 ? "it's" : "they're";
	return everything
		? translate(
				"Couldn't reach {{list}}. The phone can do everything there once {{they}} connected again.",
				{ list: list, they: they }
			)
		: translate(
				"Couldn't reach {{list}}. The phone can still do everything there until {{they}} connected again.",
				{ list: list, they: they }
			);
};

/** The devices that made no new code, and what to do. */
const noCode = (names: readonly string[]) =>
	translate("Couldn't reach {{list}}. Try again once they're connected.", {
		list: names.join(", "),
		count: names.length,
	});

/** The names of the devices that let `phone` do everything. */
const doesEverything = (phone: PairedPhone) =>
	phone.devices
		.filter((d) => d.everything)
		.map((d) => d.name)
		.join(", ");

const Phones = () => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const phones = useMemo(() => pairedPhones(view), [view]);
	// The devices each phone has no sign-in on yet, by name.
	const waits = useMemo(
		() =>
			new Map(
				phones.map((phone) => [
					phone.id,
					awaitingSignIn(view, phone.id)
						.map((device) => device.name)
						.join(", "),
				])
			),
		[view, phones]
	);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	// The phone whose card asks before it may do everything.
	const [asking, setAsking] = useState<PhoneId | null>(null);

	const remove = async (phone: string) => {
		setBusy(true);
		setNote(null);
		try {
			const failure = failureOf(await source.removePhone(phone));
			if (failure) setNote({ text: failure, error: true });
		} catch (error) {
			console.warn(translate("Couldn't remove a phone"), error);
			setNote({ text: translate("Couldn't remove it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	const renew = async (phone: PhoneId) => {
		setBusy(true);
		setNote(null);
		try {
			const result = await source.renewPhoneCodes(phone);
			if (result.status === "error") {
				setNote({
					text: failureOf(result) ?? translate("Couldn't make a new code."),
					error: true,
				});
			} else if (result.data.length > 0) {
				setNote({ text: noCode(result.data), error: true });
			} else {
				setNote({
					text: translate("Open MaxHQ on the phone within {{minutes}} minutes.", {
						minutes: CODE_MINUTES,
					}),
					error: false,
				});
			}
		} catch (error) {
			console.warn(translate("Couldn't make a new code for a phone"), error);
			setNote({ text: translate("Couldn't make a new code. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	const setEverything = async (phone: PhoneId, everything: boolean) => {
		setBusy(true);
		setNote(null);
		try {
			const result = await source.setPhoneEverything(phone, everything);
			if (result.status === "error") {
				setNote({
					text: failureOf(result) ?? translate("Couldn't change it."),
					error: true,
				});
			} else if (result.data.length > 0) {
				setNote({ text: unreached(result.data, everything), error: true });
			}
		} catch (error) {
			console.warn(translate("Couldn't change what a phone may do"), error);
			setNote({ text: translate("Couldn't change it. Try again."), error: true });
		} finally {
			setAsking(null);
			setBusy(false);
		}
	};

	return (
		<Section title={translate("Phones")} hint={<StatusNote note={note} />}>
			{phones.length === 0 ? (
				<Row title={translate("No phone is paired yet")} />
			) : (
				phones.map((phone) => (
					<Fragment key={phone.id}>
						<Row
							title={<span className="selectable">{phone.name}</span>}
							sub={
								<>
									<span className="block">
										{phone.devices.map((d) => d.name).join(", ")}
									</span>
									{phone.everything ? (
										<span className="block text-status-busy-text">
											{translate("Can do everything on ")}
											{doesEverything(phone)}
										</span>
									) : null}
								</>
							}
						>
							<ConfirmButton
								label={translate("Remove")}
								question={translate("Remove {{name}} from every device?", {
									name: phone.name,
								})}
								confirm={translate("Remove")}
								disabled={busy}
								onConfirm={() => void remove(phone.id)}
							/>
						</Row>
						{waits.get(phone.id) ? (
							<Row
								className="pl-8"
								title={
									<span className="selectable">
										{translate("Not signed in on {{names}}", {
											names: waits.get(phone.id),
										})}
									</span>
								}
								sub={translate("A new code works for {{minutes}} minutes.", {
									minutes: CODE_MINUTES,
								})}
							>
								<button
									type="button"
									className={SECONDARY}
									aria-label={translate("New code for {{name}}", {
										name: phone.name,
									})}
									disabled={busy}
									onClick={() => void renew(phone.id)}
								>
									{translate("New code")}
								</button>
							</Row>
						) : null}
						<Row
							className="pl-8"
							title={translate("Let this phone do everything")}
							sub={
								phone.everything
									? translate("Adding keys or other phones still needs this Mac.")
									: translate("Off by default")
							}
						>
							<Switch
								aria-label={translate("Let {{name}} do everything", {
									name: phone.name,
								})}
								checked={phone.everything}
								disabled={busy}
								onCheckedChange={(on) => {
									if (on) setAsking(phone.id);
									else void setEverything(phone.id, false);
								}}
							/>
						</Row>
						{asking === phone.id ? (
							<div className="border-t border-sidebar-border px-4 py-3">
								<ConsentCard
									title={translate("Let {{name}} do everything?", {
										name: phone.name,
									})}
									risk={EVERYTHING_RISK}
									details={[
										{
											title: translate("It goes to"),
											facts: phone.devices.map((d) => ({
												label: translate("Device"),
												value: d.name,
											})),
										},
									]}
									remember="It stays on until you turn it off here. A device that isn't connected doesn't get it."
									busy={busy}
									answers={[
										{
											label: translate("Keep it off"),
											primary: true,
											onAnswer: () => setAsking(null),
										},
										{
											label: translate("Let it do everything"),
											onAnswer: () => void setEverything(phone.id, true),
										},
									]}
								/>
							</div>
						) : null}
					</Fragment>
				))
			)}
		</Section>
	);
};

/** The page's headings and labels, for the settings search. */
export const PHONES_TERMS: readonly string[] = [
	"Add phone",
	"Scan the code with MaxHQ on the phone",
	"Pairing code",
	"Devices the phone can reach",
	"Pair through",
	"Address for the phone",
	"Let this phone do everything",
	"New code",
];

/** The Phones settings page. */
export const PhonesSettingsPage = () => {
	useI18n();
	const devices = usePhoneDevices().data;
	return (
		<div className="flex flex-col gap-3">
			{devices?.status === "error" ? (
				<Section title={translate("Add phone")}>
					<Row title={translate("Add phone")} sub={failureOf(devices)} />
				</Section>
			) : devices ? (
				<AddPhone devices={devices.data} />
			) : null}
			<Phones />
		</div>
	);
};

/**
 * The address a phone dials a device at, when the one the Mac uses won't
 * do from the phone, as a row of the device's Connection card. Nothing
 * shows for a device that doesn't take phones.
 */
export const PhoneAddress = ({ device }: { device: DeviceId }) => {
	useI18n();
	const { source } = useServices();
	const queries = useQueryClient();
	const found = usePhoneDevices().data;
	const [typed, setTyped] = useState<string | null>(null);
	const [note, setNote] = useState<Note | null>(null);
	const entry = found?.status === "ok" ? found.data.find((d) => d.device === device) : undefined;
	if (!entry) return null;
	const saved = entry.address ?? "";

	const commit = async () => {
		const text = typed?.trim();
		setTyped(null);
		if (text === undefined || text === saved) return;
		setNote(null);
		try {
			const failure = failureOf(await source.setPhoneAddress(device, text || null));
			setNote(
				failure
					? { text: failure, error: true }
					: { text: translate("Saved"), error: false }
			);
		} catch (error) {
			console.warn(translate("Couldn't save the address for the phone"), error);
			setNote({ text: translate("Couldn't save it. Try again."), error: true });
		} finally {
			await queries.invalidateQueries({ queryKey: [PHONE_DEVICES] });
		}
	};

	return (
		<Row
			title={translate("Address for the phone")}
			sub={entry.note ?? translate("Where a phone dials it, such as devbox.example.net:22")}
		>
			<StatusNote note={note} />
			<input
				aria-label={translate("Address for the phone")}
				value={typed ?? saved}
				placeholder={translate("The one the Mac uses")}
				onChange={(e) => setTyped(e.target.value)}
				onBlur={() => void commit()}
				onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
					if (e.key === "Enter") void commit();
				}}
				spellCheck={false}
				autoComplete="off"
				className={cn(FIELD, "w-[220px]")}
			/>
		</Row>
	);
};
