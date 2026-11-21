// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher settings, kept on this Mac since the dispatcher runs there:
// which Claude or Codex account answers the box at the top, whether it
// makes safe changes without asking, and how long a pause starts a fresh
// chat. Each saves as you make it.

import { useMemo } from "react";

import { useServices, useView } from "@/app/services";
import { setDispatcherAccount, useDispatcherAccount } from "@/header/dispatcher-prefs";
import { ActsDirectlySection, ChatGapSection } from "@/header/dispatcher/acts-setting";
import { dispatcherAccountLabel, dispatcherAccounts } from "@maxhq/shared/dispatch/plan";
import { dispatcherDevice } from "@maxhq/shared/dispatch/reach";

import { Row, Section } from "./controls";
import { type Choice, Pick as Picker } from "./snooze-presets";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** The Dispatcher page. */
export const DispatcherSettingsPage = () => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const account = useDispatcherAccount();
	const device = dispatcherDevice(view);
	const options = useMemo<Choice<string>[]>(
		() =>
			dispatcherAccounts(view, dispatcherDevice(view)).map((a) => ({
				value: a.id,
				label: dispatcherAccountLabel(a),
			})),
		[view]
	);
	return (
		<>
			<Section title={translate("Account")}>
				<Row
					title={translate("Runs as")}
					sub={translate(
						"The dispatcher reads your projects and agents, and can't change files. Without a pick it takes the first Claude account, else the first Codex one."
					)}
				>
					{account ? (
						<Picker
							label={translate("Runs as")}
							value={account.id}
							options={options}
							onChange={(id) => setDispatcherAccount(services, id)}
							className="w-[220px]"
						/>
					) : (
						<span className="text-[12px] text-muted-foreground">
							{translate("Sign in to Claude or Codex on this Mac first")}
						</span>
					)}
				</Row>
			</Section>
			{device ? (
				<>
					<ActsDirectlySection device={device} />
					<ChatGapSection device={device} />
				</>
			) : null}
		</>
	);
};
