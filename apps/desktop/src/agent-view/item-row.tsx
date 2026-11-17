// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One timeline item, drawn by the renderer for its kind.

import type { ItemBody, TimelineItem } from "@maxhq/shared/bindings";

import { TOOL_KINDS } from "@maxhq/shared/agent-view/format";
import { ApprovalCard } from "./items/approval";
import { LimitWaitCard } from "./items/limit-wait";
import { AgentAttachments, AssistantMessage, UserMessage } from "./items/message";
import {
	CompactionRow,
	ErrorRow,
	HandoffRow,
	InterruptedRow,
	NoticeRow,
	TurnSummaryRow,
	UnknownRow,
} from "./items/notices";
import { PlanCard, TodosCard } from "./items/plan";
import { QuestionCard } from "./items/question";
import { Reasoning } from "./items/reasoning";
import { SubAgentCard } from "./items/subagent";
import { UserRunCard } from "./items/user-run";
import { VisualItem } from "@/visuals/visual-item";
import {
	CommandRow,
	FileChangeRow,
	FileReadRow,
	OpenedFileRow,
	SearchRow,
	ToolCallRow,
	WebRow,
} from "./items/tools";

type Kind = ItemBody["kind"];

/** A rough height for an item before it's measured. */
export const estimateHeight = (kind: Kind): number => {
	if (TOOL_KINDS.has(kind) || kind === "reasoning") return 27;
	switch (kind) {
		case "approval":
		case "question":
		case "todos":
		case "plan":
			return 160;
		case "subagent":
		case "limit_wait":
			return 64;
		case "compaction":
		case "interrupted":
		case "turn_summary":
		case "notice":
		case "handoff":
		case "user_run":
		case "unknown":
			return 24;
		case "agent_opened_file":
		case "agent_visual":
			return 27;
		default:
			return 90;
	}
};

/** The renderer for an item's kind. */
export const ItemBodyView = ({ item }: { item: TimelineItem }) => {
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
			return <ApprovalCard item={item} />;
		case "question":
			return <QuestionCard item={item} />;
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
			return <VisualItem item={item} />;
		case "user_run":
			return <UserRunCard item={item} />;
		case "unknown":
			return <UnknownRow item={item} />;
	}
};
