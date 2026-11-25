// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Firebase key the devices sign their pushes to Android phones with:
// the service account JSON Firebase saved, which names its project and
// holds the private key. Only a device whose `maxd` pushes to phones and
// knows FCM can take it; an older one says it needs an update. The file
// comes from the picker or a drop, the source reads it on the Rust side
// and sends it to each ticked device, and once every device has it the
// page offers to move the file to the Trash.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { useNow } from "@maxhq/shared/lib/clock";
import { fcmLine } from "@maxhq/shared/settings/login-parts";
import { FCM_STEPS } from "@maxhq/shared/settings/login-steps";
import { isLive } from "@maxhq/shared/workspaces/state";

import { PRIMARY, Section, StatusNote } from "./controls";
import {
	DeviceTicks,
	HeldList,
	KeyFileField,
	type Outcome,
	Outcomes,
	TrashOffer,
	useTicks,
	useTrash,
} from "./login-parts";
import { StepsOf } from "./login-steps";
import { fcmTargets, heldOf } from "./logins";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const Send = () => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const targets = fcmTargets(view, now);
	const [keyPath, setKeyPath] = useState<string | null>(null);
	const [ticked, setTicked] = useTicks(targets);
	const [busy, setBusy] = useState(false);
	const [outcomes, setOutcomes] = useState<Outcome[]>([]);
	const [trash, setTrash, move] = useTrash(() => setKeyPath(null));
	const [note, setNote] = useState<Note | null>(null);

	const devices = [...ticked];
	const ready = keyPath !== null && devices.length > 0 && !busy;

	const send = async () => {
		if (!ready || keyPath === null) return;
		setBusy(true);
		setNote(null);
		setOutcomes([]);
		setTrash(null);
		try {
			const results = await source.setFcmKey(devices, keyPath);
			const done = devices.map((device, i) => {
				const result = results[i];
				return {
					device,
					failure: result ? failureOf(result) : translate("The device didn't answer"),
				};
			});
			setOutcomes(done);
			if (done.every((o) => o.failure === null)) setTrash({ step: "offer", path: keyPath });
		} catch (error) {
			console.warn(translate("Couldn't send the Firebase key"), error);
			setNote({
				text: translate("Couldn't send it. Check the file and try again."),
				error: true,
			});
		} finally {
			setBusy(false);
		}
	};

	return (
		<Section
			title={translate("Send the Firebase key")}
			hint={translate("To each device you tick")}
		>
			<form
				className="flex flex-col gap-4 px-4 py-3.5"
				onSubmit={(e) => {
					e.preventDefault();
					void send();
				}}
			>
				<KeyFileField
					label={translate("Service account key")}
					path={keyPath}
					onPath={setKeyPath}
					onFailure={setNote}
					empty={translate("Drop the service account .json file here, or choose it")}
				/>

				<DeviceTicks
					targets={targets}
					ticked={ticked}
					onChange={setTicked}
					disabled={busy}
					label={translate("Devices")}
					empty={translate("No device pushes to phones yet. Update its maxd first.")}
				/>

				<div className="flex items-start gap-3">
					<div className="flex min-w-0 flex-1 flex-col gap-2">
						<StatusNote note={note} className="text-xs" />
						<Outcomes
							view={view}
							outcomes={outcomes}
							done={() =>
								translate("It pushes to your Android phones with this key.")
							}
						/>
						{trash ? (
							<TrashOffer trash={trash} onMove={() => void move(trash.path)} />
						) : null}
					</div>
					<button type="submit" className={PRIMARY} disabled={!ready}>
						{busy ? translate("Sending") : translate("Send")}
					</button>
				</div>
			</form>
		</Section>
	);
};

/** The card's headings and labels, for the settings search. */
export const FCM_LOGIN_TERMS: readonly string[] = [
	"Send the Firebase key",
	"Service account key",
	"Firebase keys devices hold",
];

/** The Android pushes card. */
export const FcmLogin = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<div className="flex flex-col gap-[18px]">
			<StepsOf of={FCM_STEPS} />
			<Send />
			<Section
				title={translate("Firebase keys devices hold")}
				hint={translate("Forget deletes the key on the device")}
			>
				<HeldList
					view={view}
					held={heldOf(view, "fcm")}
					now={now}
					live={(device) => isLive(view, device, now)}
					what={translate("the Firebase key")}
					empty={translate("No device holds a Firebase key yet.")}
					describe={fcmLine}
				/>
			</Section>
		</div>
	);
};
