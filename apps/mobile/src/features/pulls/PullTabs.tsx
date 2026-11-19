// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A PR's Files, Checks and Conversation tabs, read only, as on the
// desktop. Files come a page at a time, each with its diff in the phone's
// diff view. Checks group as failing, pending, passing and skipped.
// Replying and line reviews go through GitHub. Pulling a tab down reads it
// again.

import { formatNumber, t, useI18n } from "@maxhq/shared/i18n";

import { memo, type ReactNode, useMemo } from "react";
import { FlatList, ScrollView, StyleSheet, Text, View } from "react-native";

import type { DeviceId, PullCheck, PullComment, PullFile, PullKey } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	CHECK_GROUPS,
	CHECK_STATE,
	checksLabel,
	FILE_STATUS,
	pullFileEdit,
} from "@maxhq/shared/pulls/look";
import { readOf } from "@maxhq/shared/pulls/read";
import { pullUrl } from "@maxhq/shared/pulls/urls";
import { formatWhen } from "@maxhq/shared/status/format";

import { useServices } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { MarkdownText } from "../../ui/MarkdownText";
import { usePullToRefresh } from "../../ui/Refresh";
import { PatchView } from "../agent/PatchView";
import { ChecksMark, OutLink, PullSize, UnknownChecksBanner } from "./look";
import { pullId } from "./pulls";
import { usePaged, usePullDrops, useRead } from "./read";

/** A tab's loading line, or its failure in words. */
export const Status = ({ error, what }: { error: string | null; what: string }) => {
	const { t } = useI18n();
	return error ? (
		<Text style={[styles.quiet, styles.error]} accessibilityRole="alert">
			{t("Couldn't read the {{what}}. {{message}}", { what: what, message: error })}
		</Text>
	) : (
		<Text style={styles.quiet}>{t("Reading the {{what}}", { what: what })}</Text>
	);
};

export const Quiet = ({ children }: { children: ReactNode }) => (
	<Text style={styles.quiet}>{children}</Text>
);

/**
 * One changed file with its diff, or a line saying there's none: GitHub's
 * words for a PR, `noPatch` for any other diff.
 */
export const FileRow = memo(({ file, noPatch }: { file: PullFile; noPatch?: string }) => {
	useI18n();
	const changes = useMemo(() => [pullFileEdit(file)], [file]);
	return (
		<View style={styles.file}>
			<View style={styles.fileHead}>
				<Text style={styles.fileStatus}>{FILE_STATUS[file.status]}</Text>
				<Text style={styles.path} numberOfLines={1} ellipsizeMode="head">
					{file.path}
				</Text>
				<PullSize pull={file} />
			</View>
			{file.patch ? (
				<PatchView changes={changes} />
			) : (
				<Quiet>{noPatch ?? t("GitHub shows no diff for this file.")}</Quiet>
			)}
		</View>
	);
});
FileRow.displayName = "PullFileRow";

type TabProps = { device: DeviceId; pull: PullKey };

/** The PR's changed files, a page at a time. */
export const PullFilesTab = ({ device, pull }: TabProps) => {
	const { t } = useI18n();

	const { source } = useServices();
	const files = usePaged(
		`${device}\n${pullId(pull)}`,
		async (page) => {
			const got = readOf(
				await source.pullFiles(device, pull.repo, pull.number, page),
				"pull_files"
			);
			return { items: got.files, more: got.more };
		},
		usePullDrops()
	);
	const refresh = usePullToRefresh(files.reload);
	return (
		<FlatList
			data={files.data ?? []}
			keyExtractor={(file) => file.path}
			contentContainerStyle={styles.content}
			refreshControl={refresh}
			ListHeaderComponent={
				<OutLink url={`${pullUrl(pull)}/files`}>{t("Open on GitHub")}</OutLink>
			}
			renderItem={({ item }) => <FileRow file={item} />}
			ListEmptyComponent={
				files.data ? (
					<Quiet>{t("No files changed")}</Quiet>
				) : (
					<Status error={files.error} what="files" />
				)
			}
			ListFooterComponent={
				files.data ? (
					<>
						{files.more ? (
							<View style={styles.more}>
								<Button
									label={
										files.reading
											? t("Reading more files")
											: t("Show more files")
									}
									small
									disabled={files.reading}
									onPress={files.next}
								/>
							</View>
						) : null}
						{files.nextError ? (
							<Status error={files.nextError} what="next files" />
						) : null}
					</>
				) : undefined
			}
		/>
	);
};

const CheckRow = ({ check }: { check: PullCheck }) => {
	const { t } = useI18n();
	return (
		<View style={styles.check}>
			<Text style={styles.checkName} numberOfLines={2}>
				{check.name}
			</Text>
			<Text style={[styles.checkState, check.state === "failing" && styles.error]}>
				{CHECK_STATE[check.state]}
			</Text>
			{check.url ? <OutLink url={check.url}>{t("Details")}</OutLink> : null}
		</View>
	);
};

/** The PR's checks on its head, grouped by how each stands. */
export const PullChecksTab = ({ device, pull }: TabProps) => {
	const { t } = useI18n();

	const { source } = useServices();
	const checks = useRead(
		`${device}\n${pullId(pull)}`,
		async () => {
			const { state, checks } = readOf(
				await source.pullChecks(device, pull.repo, pull.number),
				"pull_checks"
			);
			return { state, checks };
		},
		usePullDrops()
	);
	const refresh = usePullToRefresh(checks.reload);

	const body = (): ReactNode => {
		if (!checks.data) return <Status error={checks.error} what="checks" />;
		const { state, checks: all } = checks.data;
		if (state === "unknown") return <UnknownChecksBanner repo={pull.repo} device={device} />;
		if (all.length === 0) return <Quiet>{t("No checks on this pull request")}</Quiet>;
		return (
			<>
				<View style={styles.overall}>
					<ChecksMark state={state} />
					<Text style={styles.overallText}>{checksLabel(state)}</Text>
				</View>
				{CHECK_GROUPS.map(({ state: group, title }) => {
					const inGroup = all.filter((check) => check.state === group);
					if (inGroup.length === 0) return null;
					return (
						<View key={group} accessibilityLabel={title} style={styles.group}>
							<Text style={styles.heading} accessibilityRole="header">
								{title} {formatNumber(inGroup.length)}
							</Text>
							{inGroup.map((check) => (
								<CheckRow key={`${check.name}:${check.url ?? ""}`} check={check} />
							))}
						</View>
					);
				})}
			</>
		);
	};
	return (
		<ScrollView contentContainerStyle={styles.content} refreshControl={refresh}>
			{body()}
		</ScrollView>
	);
};

const CommentCard = memo(({ comment, now }: { comment: PullComment; now: number }) => {
	useI18n();
	return (
		<View style={styles.comment}>
			<View style={styles.commentHead}>
				<Text style={styles.author} numberOfLines={1}>
					{comment.author}
				</Text>
				<Text style={styles.when}>{formatWhen(comment.at, now)}</Text>
				{comment.path ? (
					<Text style={styles.where} numberOfLines={1} ellipsizeMode="head">
						{comment.line === null ? comment.path : `${comment.path}:${comment.line}`}
					</Text>
				) : null}
			</View>
			<MarkdownText text={comment.body} />
		</View>
	);
});
CommentCard.displayName = "PullCommentCard";

/** The PR's conversation, oldest first, with a link to reply on GitHub. */
export const PullConversationTab = ({ device, pull }: TabProps) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(60_000);
	const comments = useRead(
		`${device}\n${pullId(pull)}`,
		async () =>
			readOf(await source.pullComments(device, pull.repo, pull.number), "pull_comments")
				.comments,
		usePullDrops()
	);
	const refresh = usePullToRefresh(comments.reload);
	return (
		<FlatList
			data={comments.data ?? []}
			keyExtractor={(comment) => String(comment.id)}
			contentContainerStyle={styles.content}
			refreshControl={refresh}
			ListHeaderComponent={<OutLink url={pullUrl(pull)}>{t("Reply on GitHub")}</OutLink>}
			renderItem={({ item }) => <CommentCard comment={item} now={now} />}
			ListEmptyComponent={
				comments.data ? (
					<Quiet>{t("No comments yet")}</Quiet>
				) : (
					<Status error={comments.error} what="conversation" />
				)
			}
		/>
	);
};

const styles = StyleSheet.create({
	content: { gap: 14, padding: 14, paddingBottom: 40 },
	quiet: { color: color.mutedForeground, fontSize: size.small },
	error: { color: color.errorText },
	file: { gap: 6 },
	fileHead: { flexDirection: "row", alignItems: "center", gap: 8 },
	fileStatus: { color: color.mutedForeground, fontSize: size.meta },
	path: {
		flex: 1,
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.meta,
	},
	more: { alignItems: "flex-start" },
	overall: { flexDirection: "row", alignItems: "center", gap: 6 },
	overallText: { color: color.secondaryForeground, fontSize: size.small },
	group: { gap: 2 },
	heading: {
		color: color.secondaryForeground,
		fontSize: size.tiny,
		fontWeight: "600",
		paddingBottom: 2,
	},
	check: { minHeight: 32, flexDirection: "row", alignItems: "center", gap: 8 },
	checkName: { flex: 1, color: color.foreground, fontSize: size.small },
	checkState: { color: color.mutedForeground, fontSize: size.meta },
	comment: {
		gap: 6,
		paddingHorizontal: 12,
		paddingVertical: 10,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.card,
	},
	commentHead: { flexDirection: "row", alignItems: "baseline", gap: 8 },
	author: { flexShrink: 0, color: color.foreground, fontSize: size.tiny, fontWeight: "600" },
	when: { color: color.mutedForeground, fontSize: size.tiny },
	where: {
		flex: 1,
		color: color.mutedForeground,
		fontFamily: font.mono,
		fontSize: size.tiny,
	},
});
