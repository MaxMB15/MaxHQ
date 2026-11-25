// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The New agent sheet, as the desktop's form: where it runs, which login
// and model, its effort and the mode it starts in, its branch in a
// container, and an optional title and first message. It opens from the
// inbox's More menu, or from a project's Plus with that project picked.
// Until you pick where it runs, it reads each candidate's load every 5
// seconds and puts the agent on the freest. Starting needs the Mac's
// switch for this phone on the device, so Start stays in view, off, until
// the Mac turns it on. Once maxd says the agent started, its screen takes
// the sheet's place.

import { useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import {
	balanceCandidates,
	balanceNote,
	type Choices,
	inContainer,
	pickBalanced,
	pickChoices,
	planStart,
	resolve,
} from "@maxhq/shared/new-agent/spec";
import { modeNote, startNewAgent, useLoads } from "@maxhq/shared/new-agent/start";
import { agreeOnMode } from "@maxhq/shared/settings/auto-mode";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Header } from "../../ui/Header";
import { askAutoDefault, autoConsent } from "../agent/auto-consent";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { AgentPickers } from "../tasks/AgentPickers";
import { Hint, Part, TextField } from "../tasks/fields";
import { Picker } from "../tasks/Picker";
import { BranchField } from "./BranchField";
import { DEFAULT_EFFORT, effortOptions, effortToSend, initialChoices, withEffort } from "./form";

export const NewAgentScreen = ({ navigation, route }: ScreenProps<"NewAgent">) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const [choices, setChoices] = useState<Choices>(() =>
		initialChoices(services.view.getState().view, route.params?.project ?? null)
	);
	// The effort level you picked, or null for the device's default.
	const [effort, setEffort] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);

	const candidates = useMemo(() => balanceCandidates(view, choices), [view, choices]);
	const loads = useLoads(source, candidates);
	// The clock here moves every 30 seconds, so a sample just in counts as now.
	const balanced = useMemo(
		() =>
			pickBalanced(
				view,
				candidates,
				loads,
				Math.max(now, ...loads.map((l) => l.received_at))
			),
		[view, candidates, loads, now]
	);
	const r = resolve(view, choices, balanced);
	const plan = planStart(view, choices, balanced);
	const efforts = effortOptions(view, r);
	const level = effortToSend(efforts, effort);
	const allowed = useLetsPhone(r.device);
	const note = modeNote(view, choices, r);

	const pick = (patch: Partial<Choices>) => {
		setChoices((c) => pickChoices(view, c, patch, balanced));
		setFailure(null);
	};

	const start = async () => {
		if (!plan.ok || busy || !allowed || !r.device) return;
		const device = r.device;
		setBusy(true);
		setFailure(null);
		// An Auto you didn't pick, as the device's default, asks first.
		const outcome = await startNewAgent(withEffort(plan.spec, level), r.mode, {
			agree: (mode) =>
				agreeOnMode(mode, autoConsent, askAutoDefault, {
					source,
					view: services.view.getState().view,
					device,
					agent: null,
				}),
			start: (spec) => source.startAgent(spec),
		});
		if ("agent" in outcome) {
			navigation.replace("Agent", { agent: outcome.agent });
			return;
		}
		setFailure(outcome.failure);
		setBusy(false);
	};

	const status = failure ?? (plan.ok ? null : plan.reason);
	return (
		<View style={styles.screen}>
			<Header
				title={t("New agent")}
				sheet
				back={{ label: t("Cancel"), onPress: () => navigation.goBack(), close: true }}
			/>
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				keyboardDismissMode="interactive"
				automaticallyAdjustKeyboardInsets
			>
				<Hint>{t("MaxHQ starts it and holds it, and its screen opens once it's up.")}</Hint>

				<Part label={t("New agent")}>
					<AgentPickers
						view={view}
						choices={choices}
						resolved={r}
						now={now}
						onPick={pick}
					/>
					{r.balanced && r.device ? (
						<Hint>
							{balanceNote(view, r.device)}
							{t(". Pick a place to choose yourself.")}
						</Hint>
					) : null}
					{note ? <Hint>{note}</Hint> : null}
					{efforts.length > 0 ? (
						<Picker
							label={t("Effort")}
							value={level ?? DEFAULT_EFFORT}
							options={efforts}
							onChange={(value) => {
								setEffort(value === DEFAULT_EFFORT ? null : value);
								setFailure(null);
							}}
						/>
					) : null}
					{r.project && r.workspace && inContainer(r.workspace) ? (
						<BranchField
							project={r.project}
							workspace={r.workspace.id}
							value={choices.branch}
							onChange={(branch) => pick({ branch })}
						/>
					) : null}
				</Part>

				<Part label={t("Message")}>
					<TextField
						label={t("Title")}
						value={choices.title}
						onChange={(title) => pick({ title })}
						placeholder={t("Optional. Otherwise the CLI or the first message names it")}
					/>
					<TextField
						label={t("First message")}
						value={choices.first}
						onChange={(first) => pick({ first })}
						placeholder={t("Optional. What should it work on?")}
						lines={4}
					/>
				</Part>

				{status ? (
					<Text
						style={[styles.status, failure !== null && styles.failed]}
						accessibilityRole={failure !== null ? "alert" : undefined}
						accessibilityLiveRegion="polite"
					>
						{status}
					</Text>
				) : null}
				<Button
					label={busy ? t("Starting") : t("Start agent")}
					tone="primary"
					disabled={!plan.ok || busy || !allowed}
					onPress={() => void start()}
				/>
				{r.device !== null && !allowed ? <NeedsSwitch /> : null}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	status: { color: color.mutedForeground, fontSize: size.small },
	failed: { color: color.errorText },
});
