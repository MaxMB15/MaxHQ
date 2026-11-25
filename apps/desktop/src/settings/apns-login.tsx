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
import { apnsLine } from "@maxhq/shared/settings/login-parts";
import { isLive } from "@maxhq/shared/workspaces/state";

import { FIELD, Label, Section } from "./controls";
import { HeldList, KeySendForm, useKeySend } from "./login-parts";
import { Steps } from "./login-steps";
import { heldOf, pushTargets } from "./logins";
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
	const [keyId, setKeyId] = useState("");
	const [teamId, setTeamId] = useState("");
	const sending = useKeySend({
		targetsOf: pushTargets,
		deliver:
			keyId.trim() !== "" && teamId.trim() !== ""
				? (devices, keyPath) =>
						source.setApnsKey(devices, keyId.trim(), teamId.trim(), keyPath)
				: null,
		warning: translate("Couldn't send the APNs key"),
	});
	return (
		<KeySendForm
			sending={sending}
			title={translate("Send the key")}
			file={{
				label: translate("Private key"),
				empty: translate("Drop the AuthKey .p8 file here, or choose it"),
			}}
			noTargets={translate("No device pushes to phones yet. Update its maxd first.")}
			done={() => translate("It pushes to your phones with this key.")}
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
		</KeySendForm>
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
					describe={apnsLine}
				/>
			</Section>
		</div>
	);
};
