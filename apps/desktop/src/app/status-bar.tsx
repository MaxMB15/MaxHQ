// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The thin bar at the bottom: which build this is and whether its backend
// answers. On the sample data it also switches devbox between setups, so
// every connection state can be looked at.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import type { AppInfo } from "@maxhq/shared/bindings";
import type { DataSource, MockControls } from "@maxhq/shared/data/source";

import { useServices } from "./services";

/** What the bar says about the backend. */
export const backendText = (
	query:
		| { status: "pending" }
		| { status: "success"; data: AppInfo | null }
		| { status: "error"; error: unknown },
	kind: DataSource["kind"]
): string => {
	switch (query.status) {
		case "pending":
			return t("Starting");
		case "error":
			return t("The app's backend didn't answer. {{value1}}", {
				value1: String(query.error),
			});
		case "success": {
			const info = query.data;
			const build = info
				? t("MaxHQ {{version}}, protocol {{protocol_version}}", {
						version: info.version,
						protocol_version: formatNumber(info.protocol_version, {
							useGrouping: false,
						}),
					})
				: "";
			if (kind === "mock") {
				return info
					? t("{{build}}. Showing sample data", { build: build })
					: t("Browser preview with sample data. Commands need the desktop app.");
			}
			return build || "MaxHQ";
		}
	}
};

const ScenarioPicker = ({ mock }: { mock: MockControls }) => {
	const { t } = useI18n();

	const [scenario, setScenario] = useState(() => mock.scenario());
	return (
		<label className="ml-auto flex items-center gap-1.5">
			<span>{t("Sample setup")}</span>
			<select
				value={scenario}
				onChange={(e) => {
					mock.setScenario(e.target.value);
					setScenario(e.target.value);
				}}
				className="h-[18px] rounded-[4px] border border-input bg-background px-1 text-[11px] text-secondary-foreground"
			>
				{mock.scenarios.map((s) => (
					<option key={s.id} value={s.id}>
						{s.label}
					</option>
				))}
			</select>
		</label>
	);
};

/** The status bar. */
export const StatusBar = () => {
	useI18n();

	const { source } = useServices();
	const info = useQuery({ queryKey: ["app-info"], queryFn: () => source.appInfo() });
	return (
		<footer className="flex h-6 shrink-0 items-center gap-3 border-t border-border bg-card px-3 text-[11px] text-muted-foreground">
			<span role="status">{backendText(info, source.kind)}</span>
			{source.mock ? <ScenarioPicker mock={source.mock} /> : null}
		</footer>
	);
};
