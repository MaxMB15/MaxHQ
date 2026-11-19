// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A workflow run's menu, from a long press on its row in Workflows and the
// ⋯ on its own screen, as the desktop's: Open, Stop run while an agent
// works on it, which asks first, and Copy run ID. Stop run stops each
// agent's turn, which the phone can always ask for.

import { t } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import { Alert } from "react-native";

import type { AgentId, WorkflowRun } from "@maxhq/shared/bindings";
import { describe } from "@maxhq/shared/data/request-source";
import { stopRunQuestion, stopTargets } from "@maxhq/shared/workflow/stop";

import type { Services } from "../../services";
import { choose, confirm } from "../../ui/choose";
import { runMenu } from "./runs";

/** Stops the turns of the agents working on a run, once you say yes to what that loses. */
const askToStop = (services: Services, run: WorkflowRun, agents: readonly AgentId[]) =>
	confirm(stopRunQuestion(run, agents.length), t("Stop run"), () => {
		void Promise.all(agents.map((agent) => services.source.interrupt(agent))).then(
			(results) => {
				const failed = results.find((result) => result.status === "error");
				if (failed?.status === "error") {
					Alert.alert(t("Couldn't stop the run"), describe(failed.error));
				}
			}
		);
	});

const copyId = (run: WorkflowRun) =>
	void Clipboard.setStringAsync(run.key.run).then(
		(copied) => {
			if (!copied) Alert.alert(t("Couldn't copy"));
		},
		() => Alert.alert(t("Couldn't copy"))
	);

/** Opens a run's menu in the system's sheet; `open` adds Open, for a row. */
export const openRunMenu = (services: Services, run: WorkflowRun, open?: () => void): void => {
	const agents = stopTargets(services.view.getState().view, run);
	choose(
		run.label,
		runMenu({
			open,
			stop: agents.length > 0 ? () => askToStop(services, run, agents) : null,
			copy: () => copyId(run),
		})
	);
};
