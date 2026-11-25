// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Dispatcher card, kept on this Mac since the dispatcher runs there:
// which Claude or Codex account answers the box at the top, whether it
// makes safe changes without asking, and how long a pause starts a fresh
// chat. Each saves as you make it.

import { useMemo } from "react";

import { useServices, useViewOf } from "@/app/services";
import { setDispatcherAccount, useDispatcherAccount } from "@/header/dispatcher-prefs";
import { ActsDirectlyRow, ChatGapPart } from "@/header/dispatcher/acts-setting";
import { dispatcherAccountLabel, dispatcherAccounts } from "@maxhq/shared/dispatch/plan";
import { dispatcherDevice } from "@maxhq/shared/dispatch/reach";
import type { EntityKind } from "@maxhq/shared/view/replica";

import { Row, Section } from "./controls";
import { type Choice, Pick as Picker } from "./snooze-presets";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** The card's headings and labels, for the settings search. */
export const DISPATCHER_TERMS: readonly string[] = [
	"Account",
	"Runs as",
	"Changes",
	"Act directly",
	"Chats",
	"New chat after a pause",
];

// The tables the card reads.
const KINDS: readonly EntityKind[] = [
	"device",
	"device_link",
	"device_settings",
	"account",
	"model_server",
];

/** The Dispatcher card. */
export const DispatcherSettingsPage = () => {
	useI18n();
	const services = useServices();
	const view = useViewOf(KINDS);
	const account = useDispatcherAccount();
	const device = dispatcherDevice(view);
	const options = useMemo<Choice<string>[]>(
		() =>
			dispatcherAccounts(view, dispatcherDevice(view)).map((a) => ({
				value: a.id,
				label: dispatcherAccountLabel(a, view.tables.model_server),
			})),
		[view]
	);
	return (
		<Section title={translate("Dispatcher")}>
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
			{device ? (
				<>
					<ActsDirectlyRow device={device} />
					<ChatGapPart device={device} />
				</>
			) : null}
		</Section>
	);
};
