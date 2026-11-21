// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The editor's parts for tasks that take requests, as the desktop's: the
// signature a webhook checks, whether it holds requests while the target is
// away, the GitHub repo and events a task watches, the fields its prompt
// fills in, and the rules both share: filters, replay and coalesce keys, a
// window that restarts, the oldest request it takes and its runs a day.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import {
	allowField,
	emptyFilter,
	FILTER_OPS,
	type FilterDraft,
	type FilterOp,
	MAX_FILTERS,
	MAX_WAIT_SECS,
	promptFields,
	type TaskDraft,
	toggled,
	withFilter,
} from "@maxhq/shared/tasks/draft";
import {
	fieldKind,
	fieldsNote,
	GITHUB_EVENTS,
	GITHUB_LATENCY_NOTE,
} from "@maxhq/shared/tasks/format";

import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { IconButton } from "../../ui/Header";
import { Close, Plus } from "../../ui/Icon";
import { Toggle } from "../../ui/Toggle";
import { Tabs } from "../pulls/Tabs";
import { Chips, type Edit, Hint, Label, Notice, TextField, Tick } from "./fields";
import { Picker } from "./Picker";

/** How the webhook checks who sent a request. */
export const SignatureFields = ({
	draft,
	edit,
	hasSecret,
}: {
	draft: TaskDraft;
	edit: Edit;
	/** Whether the device holds a secret for this task already. */
	hasSecret: boolean;
}) => {
	const { t } = useI18n();
	return (
		<>
			<Tick
				label={t("Check a signature")}
				value={draft.signed}
				onChange={(signed) => edit({ signed })}
			/>
			{draft.signed ? (
				<>
					<TextField
						label={t("Signature header")}
						value={draft.header}
						onChange={(header) => edit({ header })}
						mono
					/>
					<View style={styles.field}>
						<Label>{t("Encoding")}</Label>
						<Tabs
							tabs={[
								{ value: "hex", label: "Hex" },
								{ value: "base64", label: "Base64" },
							]}
							value={draft.encoding}
							onChange={(encoding) => edit({ encoding })}
						/>
					</View>
					<TextField
						label={t("Prefix")}
						value={draft.prefix}
						onChange={(prefix) => edit({ prefix })}
						mono
					/>
					<TextField
						label={hasSecret ? t("New signing secret") : t("Signing secret")}
						value={draft.secret}
						onChange={(secret) => edit({ secret })}
						placeholder={
							hasSecret
								? t("Leave blank to keep the one the device holds")
								: undefined
						}
						secure
					/>
					{hasSecret ? null : (
						<Hint>
							{t(
								"Add the signing secret. Until the device holds one, it refuses every request. The secret goes to the device and never comes back."
							)}
						</Hint>
					)}
				</>
			) : null}
		</>
	);
};

/** Whether the webhook keeps requests while its target's device is away. */
export const HoldField = ({ draft, edit }: { draft: TaskDraft; edit: Edit }) => {
	const { t } = useI18n();
	return (
		<Tick
			label={t("Hold requests up to 24 hours while the target device is offline")}
			value={draft.hold}
			onChange={(hold) => edit({ hold })}
		/>
	);
};

const FilterRow = ({
	index,
	filter,
	onChange,
	onRemove,
}: {
	index: number;
	filter: FilterDraft;
	onChange: (filter: FilterDraft) => void;
	onRemove: () => void;
}) => {
	const { t } = useI18n();
	const name = t("Filter {{value1}}", { value1: formatNumber(index + 1) });
	return (
		<View accessibilityLabel={name} style={styles.filter}>
			<View style={styles.filterHead}>
				<Text style={styles.filterName}>{name}</Text>
				<IconButton
					label={t("Remove filter {{value1}}", { value1: formatNumber(index + 1) })}
					onPress={onRemove}
				>
					<Close size={14} />
				</IconButton>
			</View>
			<TextField
				label={t("Path")}
				value={filter.path}
				onChange={(path) => onChange({ ...filter, path })}
				placeholder="body.action"
				mono
			/>
			<Picker
				label={t("Test")}
				value={filter.op}
				options={FILTER_OPS.map((o) => ({ value: o.op, label: o.label }))}
				onChange={(op) => onChange({ ...filter, op: op as FilterOp })}
			/>
			{filter.op === "exists" ? null : (
				<TextField
					label={t("Value")}
					value={filter.value}
					onChange={(value) => onChange({ ...filter, value })}
					placeholder={filter.op === "in" ? "opened, closed" : "opened"}
					mono
				/>
			)}
		</View>
	);
};

/** The placeholders the prompt uses, which are free text, and the free text it may hold. */
export const PromptFields = ({ draft, edit }: { draft: TaskDraft; edit: Edit }) => {
	const { t } = useI18n();

	const github = draft.kind === "github_events";
	const used = promptFields(draft.prompt, github);
	// A webhook from before the list fills in every field, framed.
	const everything = draft.fields === null && !github;
	const allowed = draft.fields ?? [];
	const allow = (expression: string, on: boolean) =>
		edit({ fields: allowField(draft.fields, expression, on) });
	return (
		<>
			{everything ? (
				<Notice warn title={t("This prompt fills in every field of a request")}>
					<Text style={styles.noticeText}>
						{t(
							"Free text, such as a title or a comment, comes framed as untrusted data, but it can still try to steer the agent. The safe default leaves it out until you allow each field."
						)}
					</Text>
					<View style={styles.start}>
						<Button
							small
							label={t("Use the safe default")}
							onPress={() => edit({ fields: [] })}
						/>
					</View>
				</Notice>
			) : null}
			{used.length === 0 ? (
				<Hint>{t("The prompt uses no placeholders.")}</Hint>
			) : (
				<View accessibilityLabel={t("Fields")} style={styles.fields}>
					{used.map((field) => {
						const { expression, free } = field;
						return (
							<View key={expression} style={styles.fieldRow}>
								<View style={styles.fieldText}>
									<Text style={styles.expression} numberOfLines={2}>
										{expression}
									</Text>
									<Text style={[styles.kind, free && styles.free]}>
										{fieldKind(field, github)}
									</Text>
								</View>
								{free && !everything ? (
									<Toggle
										label={t("Allow {{expression}}", {
											expression: expression,
										})}
										value={allowed.includes(expression)}
										onChange={(on) => allow(expression, on)}
									/>
								) : null}
							</View>
						);
					})}
				</View>
			)}
			<Hint>{fieldsNote(github)}</Hint>
		</>
	);
};

/** The rules a webhook and GitHub events share. */
export const RulesFields = ({ draft, edit }: { draft: TaskDraft; edit: Edit }) => {
	const { t } = useI18n();

	const setFilter = (index: number, filter: FilterDraft | null) =>
		edit({ filters: withFilter(draft.filters, index, filter) });
	return (
		<>
			<View style={styles.field}>
				<Label>{t("Filters")}</Label>
				{draft.filters.length === 0 ? (
					<Hint>
						{t("Every request runs it. A filter skips the ones that don't match.")}
					</Hint>
				) : (
					draft.filters.map((filter, index) => (
						<FilterRow
							key={filter.id}
							index={index}
							filter={filter}
							onChange={(next) => setFilter(index, next)}
							onRemove={() => setFilter(index, null)}
						/>
					))
				)}
				<View style={styles.start}>
					<Button
						small
						label={t("Add filter")}
						icon={<Plus size={11} />}
						disabled={draft.filters.length >= MAX_FILTERS}
						onPress={() => edit({ filters: [...draft.filters, emptyFilter()] })}
					/>
				</View>
			</View>
			<TextField
				label={t("Replay key")}
				value={draft.replayKey}
				onChange={(replayKey) => edit({ replayKey })}
				placeholder="{{headers.x-github-delivery}}"
				mono
			/>
			<TextField
				label={t("Coalesce key")}
				value={draft.coalesceKey}
				onChange={(coalesceKey) => edit({ coalesceKey })}
				placeholder="{{body.pull_request.number}}"
				mono
			/>
			<TextField
				label={t("Coalesce window, seconds")}
				value={draft.coalesceSecs}
				onChange={(coalesceSecs) => edit({ coalesceSecs })}
				placeholder="120"
				keyboardType="number-pad"
			/>
			<TextField
				label={t("Skip requests older than, minutes")}
				value={draft.maxAge}
				onChange={(maxAge) => edit({ maxAge })}
				placeholder={t("No limit")}
				keyboardType="number-pad"
			/>
			<TextField
				label={t("Runs a day")}
				value={draft.dailyRuns}
				onChange={(dailyRuns) => edit({ dailyRuns })}
				placeholder={t("No cap")}
				keyboardType="number-pad"
			/>
			<Tick
				label={t("Restart the window with each request")}
				value={draft.coalesceRestart}
				onChange={(coalesceRestart) => edit({ coalesceRestart })}
			/>
			{draft.coalesceRestart ? (
				<TextField
					label={t("Longest wait, seconds")}
					value={draft.coalesceMaxSecs}
					onChange={(coalesceMaxSecs) => edit({ coalesceMaxSecs })}
					placeholder={String(MAX_WAIT_SECS.default)}
					keyboardType="number-pad"
				/>
			) : null}
			<Hint>
				{t(
					"Replay keys suppress repeat deliveries for 7 days. Requests with the same coalesce key run once, with each of them in the prompt, when the window closes. It counts from the first request, or with restart on from the last, up to the longest wait. A key with no placeholder, such as all, puts every request in one window."
				)}
			</Hint>
		</>
	);
};

/** The repo and events a GitHub events task watches. */
export const GithubFields = ({
	draft,
	edit,
	repos,
}: {
	draft: TaskDraft;
	edit: Edit;
	/** The repos the GitHub App reaches. */
	repos: readonly string[];
}) => {
	const { t } = useI18n();
	return (
		<>
			<Picker
				label={t("Repository")}
				value={draft.repo}
				options={repos.map((repo) => ({ value: repo, label: repo }))}
				onChange={(repo) => edit({ repo })}
				disabled={repos.length === 0}
			/>
			{repos.length === 0 ? (
				<Hint>
					{t(
						"Add repos to the GitHub App in Settings › Accounts › GitHub to watch them."
					)}
				</Hint>
			) : null}
			<Chips
				label={t("Events")}
				chips={GITHUB_EVENTS.map(({ event, label }) => ({
					key: event,
					label,
					on: draft.events.has(event),
				}))}
				onToggle={(event, on) => edit({ events: toggled(draft.events, event, on) })}
			/>
			<Hint>{t(GITHUB_LATENCY_NOTE)}</Hint>
			<Tick
				label={t("Only for the repo's owners, members and collaborators")}
				note={t(
					"Issues, pull requests, comments and reviews from anyone else won't run this task. Turn this off and, on a public repo, any stranger can start a run. Events with no author, such as pushes, forks and stars, run either way."
				)}
				value={draft.repoPeople}
				onChange={(repoPeople) => edit({ repoPeople })}
			/>
		</>
	);
};

const styles = StyleSheet.create({
	field: { gap: 6 },
	start: { alignSelf: "flex-start" },
	filter: {
		gap: 10,
		padding: 12,
		paddingTop: 4,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	filterHead: { flexDirection: "row", alignItems: "center", marginRight: -10 },
	filterName: { flex: 1, color: color.foreground, fontSize: size.small, fontWeight: "600" },
	noticeText: { color: color.secondaryForeground, fontSize: size.meta, lineHeight: 17 },
	fields: { gap: 2 },
	fieldRow: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: 10 },
	fieldText: { flex: 1, minWidth: 0, gap: 2 },
	expression: { color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	kind: { color: color.mutedForeground, fontSize: size.tiny },
	free: { color: color.busyText },
});
