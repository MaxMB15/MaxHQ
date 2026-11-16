// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's Setup on the phone, as the desktop shows it: what its last
// host check found, item by item, with the fix for each. Checking works
// from any phone; Apply needs the Mac's switch for this phone. The part
// that needs root is one command to copy, since the phone opens no
// Terminal on the device. A NixOS host gets the change to make instead.

import { useI18n } from "@maxhq/shared/i18n";

import { memo } from "react";
import { StyleSheet, Text, View } from "react-native";

import {
	FIX_TEXT,
	fixableNow,
	type HostGroup,
	hostGroups,
	hostReportOf,
	offersHostSpec,
	type RootPart,
	rootParts,
	sourceText,
} from "@maxhq/shared/hosts/report";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatWhen, plural } from "@maxhq/shared/status/format";

import { useServices } from "../../services";
import { color, font, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import {
	Buttons,
	CardBody,
	CopyTextButton,
	Muted,
	NoteLine,
	type SectionProps,
	useAction,
} from "./section";

const Code = ({ text }: { text: string }) => (
	<Text selectable style={styles.code}>
		{text}
	</Text>
);

const Group = memo(function Group({ group }: { group: HostGroup }) {
	const { t } = useI18n();
	return (
		<View style={styles.group} accessibilityLabel={group.label}>
			<View style={styles.groupHead}>
				<Text style={styles.groupLabel}>{group.label}</Text>
				<Text style={styles.groupCount}>
					{group.problems > 0
						? t("{{value1}} to fix", { value1: plural(group.problems, "item") })
						: t("All ok")}
				</Text>
			</View>
			{group.items.map((item) => (
				<View key={item.id} style={styles.item}>
					<View style={styles.itemTop}>
						<View
							accessible
							accessibilityRole="image"
							accessibilityLabel={item.ok ? t("Ok") : t("Needs a fix")}
							style={[
								styles.dot,
								{ backgroundColor: item.ok ? color.ready : color.error },
							]}
						/>
						<Text style={styles.itemLabel}>{item.label}</Text>
					</View>
					<Text style={[styles.status, !item.ok && styles.error]}>
						{item.status}
						{!item.ok && FIX_TEXT[item.fix.kind] ? (
							<Text style={styles.fix}>{` · ${FIX_TEXT[item.fix.kind]}`}</Text>
						) : null}
					</Text>
					{!item.ok && item.fix.kind === "nix" ? (
						<View style={styles.indent}>
							<Code text={item.fix.change} />
							<Buttons>
								<CopyTextButton label={t("Copy change")} text={item.fix.change} />
							</Buttons>
						</View>
					) : null}
				</View>
			))}
		</View>
	);
});

const RootCommand = ({ name, part }: { name: string; part: RootPart }) => {
	const { t } = useI18n();
	return (
		<View style={[styles.root, styles.line]} accessibilityLabel={t("Root command")}>
			<Text style={styles.itemLabel}>{t("Run as root on {{name}}", { name: name })}</Text>
			<Muted>
				{t(
					"Fixes {{value2}}. You type the sudo password in Terminal, and MaxHQ never sees it. Check again once it ends.",
					{ value2: part.labels.join(", ") }
				)}
			</Muted>
			<Code text={part.line} />
			<Buttons>
				<CopyTextButton label={t("Copy command")} text={part.line} />
			</Buttons>
		</View>
	);
};

/** The device's Setup section, from its last host check. */
export const DeviceSetup = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(60_000);
	const lets = useLetsPhone(device);
	const { busy, note, run } = useAction();
	if (!offersHostSpec(view, device)) return null;
	const report = hostReportOf(view, device);
	const groups = report ? hostGroups(view, report) : [];
	const fixable = report ? fixableNow(report) : [];
	const roots = rootParts(groups);

	return (
		<Card
			title={t("Setup")}
			label={t("Setup of {{name}}", { name: name })}
			note={
				report
					? t("{{value1}}. Checked {{value2}}", {
							value1: sourceText(report.source),
							value2: formatWhen(report.checked_at, now),
						})
					: t("Not checked yet")
			}
		>
			<CardBody>
				<Text style={styles.title}>
					{report
						? fixable.length > 0 || roots.length > 0
							? t("{{name}} needs some setup", { name: name })
							: t("{{name}} is set up", { name: name })
						: t("MaxHQ hasn't checked {{name}} yet", { name: name })}
				</Text>
				{fixable.length > 0 ? (
					<Muted>
						{t("Apply fixes {{value1}} without root.", {
							value1: plural(fixable.length, "item"),
						})}
					</Muted>
				) : live ? null : (
					<Muted>{t("Connect to {{name}} to check it.", { name: name })}</Muted>
				)}
				<Buttons>
					<Button
						label={report ? t("Check again") : t("Check")}
						small
						disabled={!live || busy}
						onPress={() => void run(t("Checking"), () => source.checkHost(device))}
					/>
					{fixable.length > 0 ? (
						<Button
							label={t("Apply")}
							tone="primary"
							small
							disabled={!live || busy || !lets}
							onPress={() =>
								void run(t("Applying"), () => source.applyHost(device, fixable))
							}
						/>
					) : null}
				</Buttons>
				<NoteLine note={note} />
				{fixable.length > 0 && !lets ? <NeedsSwitch /> : null}
			</CardBody>
			{groups.map((group) => (
				<Group key={group.id} group={group} />
			))}
			{roots.map((part) => (
				<RootCommand key={part.line} name={name} part={part} />
			))}
		</Card>
	);
};

const styles = StyleSheet.create({
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	title: { color: color.foreground, fontSize: size.body, fontWeight: "600" },
	group: {
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
		paddingBottom: 6,
	},
	groupHead: {
		flexDirection: "row",
		alignItems: "baseline",
		justifyContent: "space-between",
		gap: 8,
		paddingHorizontal: 14,
		paddingTop: 10,
		paddingBottom: 4,
	},
	groupLabel: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "700" },
	groupCount: { color: color.mutedForeground, fontSize: size.tiny },
	item: { paddingHorizontal: 14, paddingVertical: 6, gap: 3 },
	itemTop: { flexDirection: "row", alignItems: "center", gap: 10 },
	dot: { width: 8, height: 8, borderRadius: 4 },
	itemLabel: { flexShrink: 1, color: color.foreground, fontSize: size.small },
	status: { color: color.mutedForeground, fontSize: size.meta, paddingLeft: 18 },
	error: { color: color.errorText },
	fix: { color: color.mutedForeground },
	indent: { paddingLeft: 18, gap: 6 },
	code: {
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
		paddingHorizontal: 10,
		paddingVertical: 6,
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.tiny,
	},
	root: {
		paddingHorizontal: 14,
		paddingVertical: 12,
		gap: 8,
		backgroundColor: tint(color.subpanel, 0.5),
	},
});
