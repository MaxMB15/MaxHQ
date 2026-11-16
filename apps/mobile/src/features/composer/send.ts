// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the phone's box sends a draft, step by step as the desktop's does.
// A note pill waits for a maxd that takes them. An Auto you didn't pick
// asks first. Then the message waits for its files to get ready, takes an
// excerpt of each agent on another device and a copy of each note from
// another project, and goes out once all of them are in, with each pull
// request or issue as it is now. One that can't get ready keeps the
// message in the box and says why.

import type { AgentId, DeviceId, ProjectId, SendMode } from "@maxhq/shared/bindings";
import { attachFailure, takeCopies } from "@maxhq/shared/composer/copies";
import { attempt, failureOf } from "@maxhq/shared/composer/send";
import { settled, type Staging } from "@maxhq/shared/composer/staging";
import type { DataSource } from "@maxhq/shared/data/source";
import { gitTargetOf, hasNote, keyedGitItems, markedText } from "@maxhq/shared/pills/draft";
import { gitFinder, type GitPillsStore, shownItems } from "@maxhq/shared/pills/git-pills";
import { refreshGitItems } from "@maxhq/shared/pills/git-refresh";
import type { SendGate } from "@maxhq/shared/settings/auto-mode";
import { notePillBlocked } from "@maxhq/shared/view/notes";
import type { ViewState } from "@maxhq/shared/view/replica";
import { sendsVisuals } from "@maxhq/shared/view/visuals";

import { messageInput, type PhoneDraft, unreadyFiles, wholeOf } from "./draft";

/** What a box sends with, and to whom. */
export type SendSetup = {
	readonly source: DataSource;
	readonly view: () => ViewState;
	readonly agent: AgentId;
	/** The device the agent runs on, where its files stage. */
	readonly device: DeviceId;
	/** The project the agent works in, whose notes go as pills rather than copies. */
	readonly ownProject: ProjectId | null;
	readonly staging: Staging;
	readonly gitPills: GitPillsStore;
	/** Whether the message may go, as an Auto you didn't pick asks first. */
	readonly agree: () => Promise<SendGate>;
	/** Told when the message starts and stops waiting for its files and copies. */
	readonly onWait?: (waiting: boolean) => void;
};

/** Whether a message went, and what to show under the box. */
export type Sent = { readonly sent: boolean; readonly failure: string | null };

/**
 * Sends `draft` as `mode` says. A message that went still shows why a
 * change on the way didn't go through, such as a default the phone
 * couldn't set.
 */
export const sendDraft = async (
	setup: SendSetup,
	draft: PhoneDraft,
	mode: SendMode
): Promise<Sent> => {
	const { source, agent, device, staging, gitPills } = setup;
	const whole = wholeOf(draft);
	// A maxd too old for note pills would drop them, so the message waits.
	const blocked = notePillBlocked(setup.view(), device);
	if (blocked !== null && hasNote(whole)) return { sent: false, failure: blocked };
	const gate = await setup.agree();
	if (!gate.go) return { sent: false, failure: gate.failure };
	setup.onWait?.(true);
	const [files, copies] = await Promise.all([
		settled(staging),
		takeCopies(source, setup.view, device, whole, setup.ownProject),
	]).finally(() => setup.onWait?.(false));
	const failed = unreadyFiles(whole, files);
	if (failed.length > 0) return { sent: false, failure: attachFailure(failed) };
	if (copies.failure !== null) return { sent: false, failure: copies.failure };
	// A pull request or issue picked a while ago goes out as it is now.
	const text = markedText(whole);
	const shown = shownItems(gitPills.getState(), text);
	const find = gitFinder(source, agent);
	const gitItems = await refreshGitItems(
		shown,
		(item) => find(gitTargetOf(text, item)),
		Date.now()
	);
	gitPills.getState().seed(
		keyedGitItems(
			text,
			gitItems.filter((item, index) => item !== shown[index])
		)
	);
	// An older maxd gets a visual as its page's file.
	const input = messageInput(whole, gitItems, files, copies, sendsVisuals(setup.view(), device));
	const failure = failureOf(
		await attempt(() => source.sendInput(agent, input, mode)),
		mode === "steer" ? "steer" : "send"
	);
	return failure === null ? { sent: true, failure: gate.failure } : { sent: false, failure };
};
