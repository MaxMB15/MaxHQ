// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The ACP registry as a device sees it, as the desktop shows it: a search
// over its agents, each with how it would install and whether it's in, and
// Install or Update, which need the Mac's switch for this phone. A binary
// the registry gives no checksum for asks once before it goes.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useState } from "react";
import { StyleSheet, Text } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import {
	registryOf,
	type RegistryRow,
	registryRows,
	uncheckedWarning,
} from "@maxhq/shared/hosts/registry";
import { offersToolInstalls } from "@maxhq/shared/hosts/tools";

import { useServices } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { Input } from "../../ui/Field";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { Buttons, CardBody, Muted, NoteLine, type SectionProps, useAction } from "./section";

const AgentRow = memo(function AgentRow({
	device,
	row,
	live,
	lets,
}: {
	device: DeviceId;
	row: RegistryRow;
	live: boolean;
	lets: boolean;
}) {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	const [asking, setAsking] = useState(false);
	const { agent } = row;
	const verb = row.update ? t("Update") : t("Install");
	const install = (confirmUnchecked: boolean) => {
		setAsking(false);
		void run(t("Sending"), () =>
			source.installAcpAgent(
				device,
				agent.id,
				agent.version,
				{ type: "host" },
				confirmUnchecked
			)
		);
	};
	const offers = row.installable && (agent.installed === null || row.update);
	const off = !live || busy || !lets;

	return (
		<CardBody>
			<Text style={styles.title}>{agent.name}</Text>
			{agent.description ? (
				<Text numberOfLines={3} style={styles.description}>
					{agent.description}
				</Text>
			) : null}
			{asking ? (
				<Text accessibilityLiveRegion="assertive" style={styles.warning}>
					{uncheckedWarning(agent)}
				</Text>
			) : (
				<Muted>{row.status}</Muted>
			)}
			{asking ? (
				<Buttons>
					<Button label={t("Cancel")} small onPress={() => setAsking(false)} />
					<Button
						label={t("Install anyway")}
						tone="danger"
						small
						disabled={off}
						onPress={() => install(true)}
					/>
				</Buttons>
			) : offers ? (
				<Buttons>
					<Button
						label={verb}
						hint={`${verb} ${agent.name}`}
						small
						disabled={off}
						onPress={() => (row.needsConfirm ? setAsking(true) : install(false))}
					/>
				</Buttons>
			) : null}
			<NoteLine note={note} />
		</CardBody>
	);
});

/** The ACP registry on a device, to search and install from. */
export const DeviceRegistry = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();

	const lets = useLetsPhone(device);
	const [query, setQuery] = useState("");
	const registry = registryOf(view, device);
	if (!offersToolInstalls(view, device)) return null;
	const rows = registry ? registryRows(registry, query) : [];
	return (
		<Card
			title={t("ACP registry")}
			label={t("ACP registry for {{name}}", { name: name })}
			note={t("Agents to install")}
		>
			<CardBody>
				<Input
					accessibilityLabel={t("Search the registry")}
					placeholder={t("Search by name or what it does")}
					value={query}
					onChangeText={setQuery}
					autoCapitalize="none"
					autoCorrect={false}
					returnKeyType="search"
				/>
				{registry?.problem ? (
					<Text accessibilityLiveRegion="polite" style={styles.warning}>
						{registry.problem}
					</Text>
				) : null}
				{!registry ? (
					<Muted>
						{live
							? t("{{name}} hasn't fetched the registry yet.", { name: name })
							: t("Connect to {{name}} to see the registry.", { name: name })}
					</Muted>
				) : rows.length === 0 ? (
					<Muted>
						{query.trim() ? t("No agent matches.") : t("The registry lists no agents.")}
					</Muted>
				) : null}
				{rows.length > 0 && !lets ? <NeedsSwitch /> : null}
			</CardBody>
			{rows.map((row) => (
				<AgentRow key={row.agent.id} device={device} row={row} live={live} lets={lets} />
			))}
		</Card>
	);
};

const styles = StyleSheet.create({
	title: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	description: { color: color.secondaryForeground, fontSize: size.meta },
	warning: { color: color.errorText, fontSize: size.meta },
});
