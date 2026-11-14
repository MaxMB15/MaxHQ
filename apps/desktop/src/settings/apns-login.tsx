// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The APNs key the devices sign their pushes to phones with: its Key ID,
// the Apple team it belongs to, and the `.p8` Apple saved. Only a device
// whose `maxd` pushes to phones can take it. The key comes from the picker
// or a drop, the source reads it on the Rust side and sends it to each
// ticked device, and once every device has it the page offers to move the
// file to the Trash.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { isLive } from "@maxhq/shared/workspaces/state";

import { FIELD, Label, PRIMARY, Section, StatusNote } from "./controls";
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
import { Steps } from "./login-steps";
import { heldOf, pushTargets } from "./logins";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const IdField = ({
	label,
	value,
	onChange,
	placeholder,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	placeholder: string;
}) => {
	useI18n();
	return (
		<label className="flex flex-col gap-1.5">
			<Label>{label}</Label>
			<input
				aria-label={label}
				value={value}
				onChange={(e) => onChange(e.target.value)}
				placeholder={placeholder}
				spellCheck={false}
				autoComplete="off"
				autoCapitalize="characters"
				className={cn(FIELD, "w-[220px] font-mono text-xs")}
			/>
		</label>
	);
};

const Send = () => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const targets = pushTargets(view, now);
	const [keyId, setKeyId] = useState("");
	const [teamId, setTeamId] = useState("");
	const [keyPath, setKeyPath] = useState<string | null>(null);
	const [ticked, setTicked] = useTicks(targets);
	const [busy, setBusy] = useState(false);
	const [outcomes, setOutcomes] = useState<Outcome[]>([]);
	const [trash, setTrash, move] = useTrash(() => setKeyPath(null));
	const [note, setNote] = useState<Note | null>(null);

	const devices = [...ticked];
	const ready =
		keyId.trim() !== "" &&
		teamId.trim() !== "" &&
		keyPath !== null &&
		devices.length > 0 &&
		!busy;

	const send = async () => {
		if (!ready || keyPath === null) return;
		setBusy(true);
		setNote(null);
		setOutcomes([]);
		setTrash(null);
		try {
			const results = await source.setApnsKey(devices, keyId.trim(), teamId.trim(), keyPath);
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
			console.warn(translate("Couldn't send the APNs key"), error);
			setNote({
				text: translate("Couldn't send it. Check the file and try again."),
				error: true,
			});
		} finally {
			setBusy(false);
		}
	};

	return (
		<Section title={translate("Send the key")} hint={translate("To each device you tick")}>
			<form
				className="flex flex-col gap-4 px-4 py-3.5"
				onSubmit={(e) => {
					e.preventDefault();
					void send();
				}}
			>
				<div className="flex flex-wrap gap-4">
					<IdField
						label={translate("Key ID")}
						value={keyId}
						onChange={setKeyId}
						placeholder={translate("From the key's page")}
					/>
					<IdField
						label={translate("Team ID")}
						value={teamId}
						onChange={setTeamId}
						placeholder={translate("From Membership details")}
					/>
				</div>

				<KeyFileField
					label={translate("Private key")}
					path={keyPath}
					onPath={setKeyPath}
					onFailure={setNote}
					empty={translate("Drop the AuthKey .p8 file here, or choose it")}
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
							done={() => translate("It pushes to your phones with this key.")}
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

/** The page's headings and labels, for the settings search. */
export const APNS_LOGIN_TERMS: readonly string[] = [
	"Send the key",
	"Key ID",
	"Team ID",
	"Private key",
	"Push keys devices hold",
];

/** The phone pushes page. */
export const ApnsLogin = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<div className="flex flex-col gap-[18px]">
			<Steps login="apns" />
			<Send />
			<Section
				title={translate("Push keys devices hold")}
				hint={translate("Forget deletes the key on the device")}
			>
				<HeldList
					view={view}
					held={heldOf(view, "apns")}
					now={now}
					live={(device) => isLive(view, device, now)}
					what={translate("the APNs key")}
					empty={translate("No device holds an APNs key yet.")}
					describe={(c) =>
						translate("Key {{key_id}}, team {{team_id}}.", {
							key_id: c.kind.key_id,
							team_id: c.kind.team_id,
						})
					}
				/>
			</Section>
		</div>
	);
};
