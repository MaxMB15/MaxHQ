// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The branch a new agent in a container works on, as the desktop's field:
// free text, where empty lets maxd name a new branch. Under it sit a few
// branches that match what you typed: the ones the project's agents work
// on, its open PRs' heads and the repo's other branches. A tap fills the
// field. It says when another agent in the container has the branch,
// since they'll share its folder. A device that can't list the repo's
// branches or PRs leaves the suggestions to what the phone already knows.

import { useI18n } from "@maxhq/shared/i18n";

import { useMemo } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { ProjectId, WorkspaceId } from "@maxhq/shared/bindings";
import {
	agentsOnBranch,
	branchProblem,
	branchSuggestions,
	branchTag,
	folderClash,
	SHARED_BRANCH_NOTE,
} from "@maxhq/shared/new-agent/branch";
import { readOf } from "@maxhq/shared/pulls/read";

import { useServices, useView } from "../../services";
import { color, font, radius, size, TOUCH } from "../../theme";
import { useRead } from "../pulls/read";
import { Hint, TextField } from "../tasks/fields";
import { shownBranches } from "./form";

/** A suggestion's height; its slop takes a touch to 44. */
const CHIP = 30;
const SLOP = { top: (TOUCH - CHIP) / 2, bottom: (TOUCH - CHIP) / 2 };

export const BranchField = ({
	project,
	workspace,
	value,
	onChange,
}: {
	project: ProjectId;
	workspace: WorkspaceId;
	value: string;
	onChange: (value: string) => void;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const pulls = useRead(
		`${project}\nopen`,
		async () => readOf(await source.listPulls(project, "open"), "pulls").pulls
	);
	const repo = useRead(project, async () =>
		readOf(await source.listBranches(project), "branches")
	);
	const suggestions = useMemo(
		() => branchSuggestions(view, project, pulls.data ?? [], repo.data),
		[view, project, pulls.data, repo.data]
	);
	const shown = useMemo(() => shownBranches(suggestions, value), [suggestions, value]);
	const name = value.trim();
	const bad =
		name !== "" &&
		(branchProblem(name) !== null || folderClash(view, workspace, name) !== null);
	const shared = name !== "" && !bad && agentsOnBranch(view, workspace, name).length > 0;

	return (
		<View style={styles.field}>
			<TextField
				label={t("Branch")}
				value={value}
				onChange={onChange}
				placeholder={t("A new maxhq/ branch")}
				mono
			/>
			{shown.length > 0 ? (
				<View style={styles.chips}>
					{shown.map((s) => {
						const tag = branchTag(s);
						return (
							<Pressable
								key={s.name}
								accessibilityRole="button"
								accessibilityLabel={tag ? `${s.name}, ${tag}` : s.name}
								hitSlop={SLOP}
								onPress={() => onChange(s.name)}
								style={({ pressed }) => [styles.chip, pressed && styles.pressed]}
							>
								<Text style={styles.name} numberOfLines={1}>
									{s.name}
								</Text>
								{tag ? <Text style={styles.tag}>{tag}</Text> : null}
							</Pressable>
						);
					})}
				</View>
			) : null}
			{shared ? <Hint>{t(SHARED_BRANCH_NOTE)}</Hint> : null}
		</View>
	);
};

const styles = StyleSheet.create({
	field: { gap: 8 },
	chips: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
	chip: {
		maxWidth: "100%",
		height: CHIP,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 9,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	name: { flexShrink: 1, color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	tag: { color: color.secondaryForeground, fontSize: size.tiny },
});
