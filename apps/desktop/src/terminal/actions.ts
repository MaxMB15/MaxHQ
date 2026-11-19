// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the drawer, its shortcut and the chat do with an agent's terminals:
// open a tab, close one, asking first while something runs in it, start
// an exited one again, show or hide the drawer, paste into it, take a tab
// out as a pane and back, and add picked output to a chat.

import type { Services } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { paneId, paneParams } from "@/layout/pane-id";
import { panesOf } from "@/layout/panes";
import { noticesOf } from "@/sidebar/notices";
import type { AgentId, DeviceId, TerminalId } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import { t } from "@maxhq/shared/i18n";
import { pickedOutput, runningCommand } from "@maxhq/shared/terminal/drawer";
import {
	openedTerminal,
	openShell,
	runCommand,
	type TerminalSize,
} from "@maxhq/shared/terminal/requests";
import {
	supportsTerminals,
	type TerminalEntity,
	terminalById,
	terminalsOf,
} from "@maxhq/shared/terminal/select";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";
import { deviceName } from "@maxhq/shared/view/select";
import type { ViewState } from "@maxhq/shared/view/replica";

import { drawerOf, drawersOf, setActive, setDrawer, setSplit } from "./drawer-prefs";
import { registryOf } from "./live";
import type { CommandOutput, Picked } from "./term-view";

/** The size a new tab starts at, before its view measures its place. */
const FIRST_SIZE: TerminalSize = { cols: 100, rows: 20 };

/** The device the agent runs on, if the view knows it. */
export const agentDevice = (view: ViewState, agent: AgentId): DeviceId | undefined => {
	const workspace = view.tables.agent.get(agent)?.workspace;
	return workspace ? view.tables.workspace.get(workspace)?.device : undefined;
};

/** The terminals open as panes of their own, which leave their drawer's tabs. */
export const poppedOut = (services: Services): ReadonlySet<TerminalId> =>
	new Set(
		(panesOf(services).api()?.panels ?? []).flatMap((panel) => {
			const params = paneParams(panel.id);
			return params.kind === "terminal" ? [params.terminal] : [];
		})
	);

/** The drawer's active tab: the one you picked, or the newest. */
export const activeOf = <T extends { readonly id: TerminalId }>(
	tabs: readonly T[],
	picked: TerminalId | undefined
): T | undefined => tabs.find((tab) => tab.id === picked) ?? tabs.at(-1);

const fail = (services: Services, text: string) => showFailure(noticesOf(services), text);

/**
 * Whether the agent's device has terminals. When it hasn't, says it needs
 * a newer maxd.
 */
const checkSupported = (services: Services, agent: AgentId): boolean => {
	const view = services.view.getState().view;
	const device = agentDevice(view, agent);
	if (device && supportsTerminals(view, device)) return true;
	fail(
		services,
		t("Terminals need a newer maxd on {{device}}.", {
			device: device ? deviceName(view, device) : t("Unknown device"),
		})
	);
	return false;
};

/** Opens a shell tab in the agent's drawer and makes it the active one. */
export const openTab = async (services: Services, agent: AgentId): Promise<TerminalId | null> => {
	const view = services.view.getState().view;
	const tabs = terminalsOf(view, agent);
	const like = activeOf(tabs, drawersOf(services).getState().active[agent]);
	const size = like ? { cols: like.cols, rows: like.rows } : FIRST_SIZE;
	const result = await openShell(services.source, agent, size);
	const terminal = openedTerminal(result);
	if (!terminal) {
		fail(services, failureOf(result) ?? t("Couldn't open a terminal."));
		return null;
	}
	setActive(services, agent, terminal);
	return terminal;
};

// The agents whose first shell is on its way, so a drawer that draws
// again before it shows doesn't open a second.
const opening = new Set<AgentId>();

/** Opens the first shell of a drawer that has no tabs. */
export const openFirstTab = (services: Services, agent: AgentId): void => {
	if (opening.has(agent)) return;
	opening.add(agent);
	void openTab(services, agent).finally(() => opening.delete(agent));
};

/**
 * Shows a second tab beside the active one, or one tab alone again. With
 * no other tab, it opens a shell, which takes the keys.
 */
export const toggleSplit = async (
	services: Services,
	agent: AgentId,
	tabs: readonly TerminalEntity[]
): Promise<void> => {
	const store = drawersOf(services);
	const { active, split } = store.getState();
	const current = activeOf(tabs, active[agent]);
	if (!current) return;
	const beside = split[agent];
	if (beside !== undefined && beside !== current.id && tabs.some((tab) => tab.id === beside))
		return setSplit(services, agent, null);
	const at = tabs.indexOf(current);
	const other = tabs[at + 1] ?? tabs[at - 1];
	if (other) return setSplit(services, agent, other.id);
	if (!(await openTab(services, agent))) return;
	setSplit(services, agent, current.id);
	store.setState({ focusing: agent });
};

/**
 * Closes a tab. While a command runs in it, or the device says something
 * does, it asks first and then stops it. The drawer hides with its last
 * tab. Resolves with whether it closed.
 */
export const closeTab = async (services: Services, terminal: TerminalEntity): Promise<boolean> => {
	const command = runningCommand(terminal);
	const ask = (detail: string) =>
		requestConfirm(services, {
			title: command
				? t("Stop {{command}} and close the tab?", { command })
				: t("Close the terminal?"),
			detail,
			action: t("Close"),
		});
	if (command && !(await ask(t("It's still running, and closing the tab stops it."))))
		return false;
	let result = await services.source.closeTerminal(terminal.id, command !== null);
	if (!command && result.status === "ok" && result.data.status === "refused") {
		if (!(await ask(result.data.reason))) return false;
		result = await services.source.closeTerminal(terminal.id, true);
	}
	const failure = failureOf(result);
	if (failure) {
		fail(services, failure);
		return false;
	}
	registryOf(services)?.forget(terminal.id);
	const popped = poppedOut(services);
	const left = terminalsOf(services.view.getState().view, terminal.agent).filter(
		(tab) => tab.id !== terminal.id && !popped.has(tab.id)
	);
	if (left.length === 0) setDrawer(services, terminal.agent, { open: false });
	return true;
};

/** Renames a tab, or gives it back the shell's own title when `name` is empty. */
export const renameTab = async (services: Services, terminal: TerminalId, name: string) => {
	const failure = failureOf(await services.source.renameTerminal(terminal, name));
	if (failure) fail(services, failure);
};

/**
 * Starts an exited tab over: a new shell, the same command moved to the
 * drawer, or a new session in the folder the last one ended in, in its place.
 */
export const restartTab = async (services: Services, terminal: TerminalEntity): Promise<void> => {
	const { agent, kind } = terminal;
	const size = { cols: terminal.cols, rows: terminal.rows };
	const result =
		kind.type === "run"
			? await runCommand(services.source, agent, kind.command, {
					private: kind.private,
					size,
				})
			: kind.type === "session"
				? await services.source.sessionCd(agent, terminal.cwd)
				: await openShell(services.source, agent, size);
	const next = openedTerminal(result);
	if (!next) return fail(services, failureOf(result) ?? t("Couldn't open a terminal."));
	if (kind.type === "run") await services.source.moveTerminal(next);
	const { split } = drawersOf(services).getState();
	if (split[agent] === terminal.id) setSplit(services, agent, next);
	setActive(services, agent, next);
	await services.source.closeTerminal(terminal.id, true);
	registryOf(services)?.forget(terminal.id);
};

/**
 * Shows or hides the drawer of the agent you're on, for its shortcut. A
 * closed drawer opens and its active tab takes focus, an open one takes
 * focus, and one that has it hides. Answers true when it hid, so the
 * composer takes focus.
 */
export const toggleDrawer = (services: Services): boolean => {
	const agent = services.ui.getState().selectedAgent;
	if (!agent || !checkSupported(services, agent)) return false;
	const store = drawersOf(services);
	const focused = registryOf(services)?.focused();
	const view = services.view.getState().view;
	const here = focused ? terminalById(view, focused)?.agent === agent : false;
	if (drawerOf(store.getState(), agent).open && here && !poppedOut(services).has(focused!)) {
		setDrawer(services, agent, { open: false });
		return true;
	}
	setDrawer(services, agent, { open: true });
	store.setState({ focusing: agent });
	return false;
};

/** Opens the agent's drawer on this terminal's tab, or focuses its pane. */
export const showInDrawer = (services: Services, agent: AgentId, terminal: TerminalId): void => {
	const pane = panesOf(services)
		.api()
		?.getPanel(paneId({ kind: "terminal", agent, terminal }));
	if (pane) return pane.api.setActive();
	setDrawer(services, agent, { open: true });
	setActive(services, agent, terminal);
	drawersOf(services).setState({ focusing: agent });
};

/**
 * Opens the agent's drawer and types `text` into its active tab without
 * Enter, opening a shell when there's none or the active one exited.
 */
export const pasteIntoDrawer = async (
	services: Services,
	agent: AgentId,
	text: string
): Promise<void> => {
	if (!checkSupported(services, agent)) return;
	setDrawer(services, agent, { open: true });
	const view = services.view.getState().view;
	const popped = poppedOut(services);
	const tabs = terminalsOf(view, agent).filter((tab) => !popped.has(tab.id));
	const active = activeOf(tabs, drawersOf(services).getState().active[agent]);
	const target = active?.status.state === "running" ? active.id : await openTab(services, agent);
	if (!target) return;
	setActive(services, agent, target);
	drawersOf(services).setState({ focusing: agent });
	registryOf(services)?.paste(target, text);
};

/** Takes a tab out of its drawer into a pane of its own beside the agent's. */
export const popOut = (services: Services, agent: AgentId, terminal: TerminalId): void =>
	panesOf(services).openTerminal(agent, terminal, { split: true });

/** Puts a tab back in its drawer, closing its pane. */
export const popBack = (services: Services, agent: AgentId, terminal: TerminalId): void => {
	panesOf(services)
		.api()
		?.getPanel(paneId({ kind: "terminal", agent, terminal }))
		?.api.close();
	setDrawer(services, agent, { open: true });
	setActive(services, agent, terminal);
	drawersOf(services).setState({ focusing: agent });
};

/**
 * Adds text picked in a terminal to an agent's composer as an Output pill,
 * with the lines it sits on, or what a command its shell marked printed.
 * `total` is how many lines the terminal printed.
 */
export const addPicked = (
	services: Services,
	to: AgentId,
	terminal: TerminalEntity,
	picked: Picked | CommandOutput,
	total: number,
	focus: boolean
): void => {
	const view = services.view.getState().view;
	const ref = pickedOutput(terminal, deviceName(view, terminal.device), picked, total);
	quoteInbox.send(to, ref, focus);
};
