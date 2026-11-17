// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The APNs key the devices sign their pushes to phones with, on the phone.
// The key goes to devices from the Mac, since only a file the Mac's picker
// offers can be sent and the phone has none, so the phone lists the keys
// devices hold, with Forget.

import { useI18n } from "@maxhq/shared/i18n";

import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { apnsLine } from "@maxhq/shared/settings/login-parts";
import { deviceFirst, heldOf } from "@maxhq/shared/settings/logins";
import { isLive } from "@maxhq/shared/workspaces/state";

import { useView } from "../../services";
import { Card } from "../../ui/Card";
import { CardBody, Muted } from "../devices/section";
import { HeldList, StepsCard } from "./login-parts";

/** The phone pushes page. `device` puts that device's key first. */
export const ApnsLogin = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<>
			<StepsCard login="apns" />
			<Card title={t("Send the key")} note={t("To each device you tick")}>
				<CardBody first>
					<Muted>
						{t(
							"Send the key from Settings on your Mac. The phone can't read the key file."
						)}
					</Muted>
				</CardBody>
			</Card>
			<Card
				title={t("Push keys devices hold")}
				note={t("Forget deletes the key on the device")}
			>
				<HeldList
					view={view}
					held={deviceFirst(heldOf(view, "apns"), device)}
					now={now}
					live={(d) => isLive(view, d, now)}
					what={t("the APNs key")}
					empty={t("No device holds an APNs key yet.")}
					describe={apnsLine}
				/>
			</Card>
		</>
	);
};
