// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent tools page's cards: the agents allowed to run agents and the
// access keys tools outside MaxHQ use, each to review and revoke. Granting
// a permission, making a key and the defaults a new permission starts with
// stay on the Mac. A key's file stays on its device, so the page shows
// where it is and the command that uses it, never the key.

import { t, useI18n } from "@maxhq/shared/i18n";

import { memo, useCallback, useMemo } from "react";
import { StyleSheet, Text } from "react-native";

import {
	type AgentGrantRow,
	agentRows,
	type KeyGrantRow,
	keyRows,
} from "@maxhq/shared/agent-tools/grants";
import type { AccessKey, Permission, PermissionId } from "@maxhq/shared/bindings";
import type { KeysResult, PermissionsResult } from "@maxhq/shared/data/source";
import { useNow } from "@maxhq/shared/lib/clock";
import { errorText } from "@maxhq/shared/timeline/store";

import { useServices, useView } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { Buttons, CardBody, CopyTextButton, NoteLine, useAction } from "../devices/section";
import { useRead } from "../pulls/read";

const listed = async <T,>(result: Promise<PermissionsResult | KeysResult>): Promise<T[]> => {
	const answer = await result;
	if (answer.status === "error") throw new Error(errorText(answer.error));
	return answer.data as T[];
};

/** Takes a permission off every device once you confirm, then reads the lists again. */
const useRevoke = (permission: PermissionId, reload: () => Promise<unknown>) => {
	const { source } = useServices();
	const action = useAction();
	const revoke = (question: { title: string; detail: string }) =>
		confirm(question, t("Revoke"), () => {
			void action.run(null, () => source.revokePermission(permission)).then(() => reload());
		});
	return { ...action, revoke };
};

// Each row takes its own words, so a change elsewhere in the view leaves it be.
const AgentRow = memo(function AgentRow({
	permission,
	title,
	line,
	reload,
}: AgentGrantRow & { reload: () => Promise<unknown> }) {
	useI18n();
	const { busy, note, revoke } = useRevoke(permission, reload);
	return (
		<>
			<CardRow label={title} note={line}>
				<Button
					label={t("Revoke")}
					tone="danger"
					small
					disabled={busy}
					onPress={() =>
						revoke({ title, detail: t("It can't start agents after this.") })
					}
				/>
			</CardRow>
			{note ? (
				<CardBody first>
					<NoteLine note={note} />
				</CardBody>
			) : null}
		</>
	);
});

const KeyRow = memo(function KeyRow({
	permission,
	name,
	line,
	path,
	command,
	reload,
}: Omit<KeyGrantRow, "id"> & { reload: () => Promise<unknown> }) {
	useI18n();
	const { busy, note, revoke } = useRevoke(permission, reload);
	return (
		<CardBody>
			<Text style={styles.name}>{name}</Text>
			<Text style={styles.line}>{line}</Text>
			<Text selectable numberOfLines={2} style={styles.path}>
				{path}
			</Text>
			<Buttons>
				<CopyTextButton label={t("Copy path")} text={path} />
				<CopyTextButton label={t("Copy MCP command")} text={command} />
				<Button
					label={t("Revoke")}
					tone="danger"
					small
					disabled={busy}
					onPress={() =>
						revoke({ title: name, detail: t("Tools using it stop working.") })
					}
				/>
			</Buttons>
			<NoteLine note={note} />
		</CardBody>
	);
});

/** A row that says the list is on its way, or why it didn't come, with a way to ask again. */
const Waiting = ({
	reading,
	failed,
	error,
	reload,
}: {
	reading: string;
	failed: string;
	error: string | null;
	reload: () => Promise<unknown>;
}) =>
	error ? (
		<CardRow label={failed} note={error}>
			<Button label={t("Try again")} small onPress={() => void reload()} />
		</CardRow>
	) : (
		<CardRow label={reading} />
	);

/** The agent tools page, under the header the Settings page gives it. */
export const AgentToolsPage = () => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const permissions = useRead("agent-tools-permissions", () =>
		listed<Permission>(source.listPermissions(null))
	);
	const keys = useRead("agent-tools-keys", () => listed<AccessKey>(source.listKeys(null)));
	const reloadPermissions = permissions.reload;
	const reloadKeys = keys.reload;
	const reload = useCallback(
		() => Promise.all([reloadPermissions(), reloadKeys()]),
		[reloadPermissions, reloadKeys]
	);
	const agents = useMemo(
		() => (permissions.data ? agentRows(view, permissions.data, now) : null),
		[view, permissions.data, now]
	);
	const keyList = useMemo(
		() => (keys.data ? keyRows(view, keys.data, permissions.data ?? [], now) : null),
		[view, keys.data, permissions.data, now]
	);

	return (
		<>
			<Card>
				<CardRow first label={t("New permissions")}>
					<Text style={styles.value}>{t("On the Mac")}</Text>
				</CardRow>
			</Card>
			<Card
				title={t("Agents allowed to run agents")}
				note={t(
					"They can start, message and stop agents within their bounds. Allow another from its agent on the Mac."
				)}
			>
				{agents === null ? (
					<Waiting
						reading={t("Reading permissions")}
						failed={t("Couldn't read the permissions. Try again.")}
						error={permissions.error}
						reload={reloadPermissions}
					/>
				) : agents.length === 0 ? (
					<CardRow label={t("None yet.")} />
				) : (
					agents.map((row) => <AgentRow key={row.permission} {...row} reload={reload} />)
				)}
			</Card>
			<Card title={t("Access keys")} note={t("For tools outside MaxHQ, such as a bot")}>
				{keyList === null ? (
					<Waiting
						reading={t("Reading the keys")}
						failed={t("Couldn't read the keys")}
						error={keys.error}
						reload={reloadKeys}
					/>
				) : keyList.length === 0 ? (
					<CardRow label={t("No keys yet")} />
				) : (
					keyList.map(({ id, ...row }) => <KeyRow key={id} {...row} reload={reload} />)
				)}
				<CardRow label={t("New access key")}>
					<Text style={styles.value}>{t("On the Mac")}</Text>
				</CardRow>
			</Card>
		</>
	);
};

const styles = StyleSheet.create({
	value: { color: color.mutedForeground, fontSize: size.small },
	name: { color: color.foreground, fontSize: size.small },
	line: { color: color.mutedForeground, fontSize: size.meta },
	path: { color: color.secondaryForeground, fontFamily: font.mono, fontSize: size.meta },
});
