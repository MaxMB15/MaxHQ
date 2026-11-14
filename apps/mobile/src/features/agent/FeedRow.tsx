// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One row of an agent's feed, drawn by the renderer for its kind as on the
// desktop, and the space above it. A row reads its own item from the store, so
// a reply streaming in redraws only that reply. The agent screen and the
// dispatcher's sheet both show them.

import { useI18n } from "@maxhq/shared/i18n";

import { memo } from "react";
import { View } from "react-native";
import { useStore } from "zustand";
import type { StoreApi } from "zustand/vanilla";

import type { TimelineItem } from "@maxhq/shared/bindings";
import { dismissBlocked } from "@maxhq/shared/agent-view/dismiss";
import { answerPlace } from "@maxhq/shared/pills/answer";
import { cardChanges, type FeedCard } from "@maxhq/shared/timeline/output";

import { useView } from "../../services";
import { AttentionCard } from "../attention/AttentionCard";
import { promptOfItem } from "../attention/prompt";
import { useFeed } from "./feed-context";
import { FoundRow } from "./FeedFind";
import type { FeedGoal } from "./find";
import { LimitWaitCard } from "./LimitWait";
import { AgentAttachments, AssistantMessage, UserMessage } from "./Messages";
import {
	CompactionRow,
	ErrorRow,
	HandoffRow,
	InterruptedRow,
	NoticeRow,
	TurnSummaryRow,
	UnknownRow,
} from "./Notices";
import { PlanCard, TodosCard } from "./Plans";
import { Reasoning } from "./Reasoning";
import { SubAgentCard } from "./SubAgentCard";
import {
	CommandRow,
	FileChangeRow,
	FileReadRow,
	OpenedFileRow,
	SearchRow,
	ToolCallRow,
	VisualRow,
	WebRow,
} from "./ToolRows";

// An approval or a question, which a session MaxHQ runs can dismiss.
const Card = ({ item }: { item: FeedCard }) => {
	const { agent, vendor, device, store, readOnly } = useFeed();
	const event = useView((s) => s.view.tables.attention.get(item.attention));
	const changes = useStore(store, (s) => cardChanges(s, item));
	const blocked = useView((s) =>
		dismissBlocked(s.view, answerPlace(s.view, agent)?.device ?? null)
	);
	return (
		<AttentionCard
			prompt={promptOfItem(item, event)}
			vendor={vendor}
			place={device}
			changes={changes}
			dismiss={readOnly ? undefined : { blocked }}
		/>
	);
};

const Body = ({ item }: { item: TimelineItem }) => {
	switch (item.kind) {
		case "user_message":
			return <UserMessage item={item} />;
		case "assistant_message":
			return <AssistantMessage item={item} />;
		case "reasoning":
			return <Reasoning item={item} />;
		case "command":
			return <CommandRow item={item} />;
		case "file_change":
			return <FileChangeRow item={item} />;
		case "file_read":
			return <FileReadRow item={item} />;
		case "search":
			return <SearchRow item={item} />;
		case "web":
			return <WebRow item={item} />;
		case "tool_call":
			return <ToolCallRow item={item} />;
		case "subagent":
			return <SubAgentCard item={item} />;
		case "plan":
			return <PlanCard item={item} />;
		case "todos":
			return <TodosCard item={item} />;
		case "approval":
		case "question":
			return <Card item={item} />;
		case "error":
			return <ErrorRow item={item} />;
		case "limit_wait":
			return <LimitWaitCard item={item} />;
		case "compaction":
			return <CompactionRow item={item} />;
		case "interrupted":
			return <InterruptedRow />;
		case "turn_summary":
			return <TurnSummaryRow item={item} />;
		case "notice":
			return <NoticeRow item={item} />;
		case "handoff":
			return <HandoffRow item={item} />;
		case "agent_attachments":
			return <AgentAttachments item={item} />;
		case "agent_opened_file":
			return <OpenedFileRow item={item} />;
		case "agent_visual":
			return <VisualRow item={item} />;
		case "unknown":
			return <UnknownRow item={item} />;
	}
};

/** One item of the feed around it, by ID, marked when `goal` names it. */
export const FeedItem = memo(
	({ id, gap, goal }: { id: string; gap: number; goal?: StoreApi<FeedGoal> }) => {
		useI18n();
		const { store } = useFeed();
		const item = useStore(store, (s) => s.items.get(id));
		if (!item) return null;
		// A reply that ended up with nothing to say.
		if (item.kind === "assistant_message" && item.text === "" && !item.streaming) return null;
		return (
			// The space sits outside the find mark, which reaches past the row.
			<View style={{ marginTop: gap }}>
				{goal ? (
					<FoundRow goal={goal} id={id}>
						<Body item={item} />
					</FoundRow>
				) : (
					<Body item={item} />
				)}
			</View>
		);
	}
);
FeedItem.displayName = "FeedItem";
