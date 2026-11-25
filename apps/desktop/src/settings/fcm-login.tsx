// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Firebase key the devices sign their pushes to Android phones with:
// the service account JSON Firebase saved, which names its project and
// holds the private key. Only a device whose `maxd` pushes to phones and
// knows FCM can take it; an older one says it needs an update. The file
// comes from the picker or a drop, the source reads it on the Rust side
// and sends it to each ticked device, and once every device has it the
// page offers to move the file to the Trash.

import { useServices, useView } from "@/app/services";
import { useNow } from "@maxhq/shared/lib/clock";
import { fcmLine } from "@maxhq/shared/settings/login-parts";
import { FCM_STEPS } from "@maxhq/shared/settings/login-steps";
import { isLive } from "@maxhq/shared/workspaces/state";

import { Section } from "./controls";
import { HeldList, KeySendForm, useKeySend } from "./login-parts";
import { StepsOf } from "./login-steps";
import { fcmTargets, heldOf } from "./logins";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const Send = () => {
	useI18n();
	const { source } = useServices();
	const sending = useKeySend({
		targetsOf: fcmTargets,
		deliver: (devices, keyPath) => source.setFcmKey(devices, keyPath),
		warning: translate("Couldn't send the Firebase key"),
	});
	return (
		<KeySendForm
			sending={sending}
			title={translate("Send the Firebase key")}
			file={{
				label: translate("Service account key"),
				empty: translate("Drop the service account .json file here, or choose it"),
			}}
			noTargets={translate("No device pushes to phones yet. Update its maxd first.")}
			done={() => translate("It pushes to your Android phones with this key.")}
		/>
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
