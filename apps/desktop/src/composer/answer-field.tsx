// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The field you answer an agent in, or ask the dispatcher in. It's the
// composer's own text box, so it has the composer's spell check and pills:
// `@` names an agent or a note of the agent's project, `#` a pull request
// or issue in its repo, and an agent, a note or a quote dragged onto it goes
// in at the caret. It grows with the text up to eight lines and then
// scrolls, and once it scrolls a button opens it taller, to most of the
// window. An answer carries no files, so a file pasted or dropped on it
// goes to the composer in its pane, as if dropped there, or stays out with
// a line that says why where there's none. A pill it can't take goes in as
// its words, each with a line that says why. The dispatcher's field keeps
// to one line while you're elsewhere. Enter sends, Shift+Enter breaks the
// line, and Enter during an IME composition stays with the composition.

import type { Agent, AgentId, DeviceId } from "@maxhq/shared/bindings";
import { FIELD_LINES, fieldMaxHeight } from "@maxhq/shared/dispatch/field";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	ANY_PLACE,
	type AnswerPlace,
	answerPlace,
	draftOf,
	NO_FILES,
	takenPiece,
	type Typed,
	typedOf,
} from "@maxhq/shared/pills/answer";
import { agentPillOf } from "@maxhq/shared/pills/agent-pill";
import { encodeFragment, readFragment } from "@maxhq/shared/pills/clipboard";
import {
	type AgentRef,
	type FileFacts,
	gitNumbers,
	keyedGitItems,
	markedText,
	plainText,
	type Segment,
} from "@maxhq/shared/pills/draft";
import type { TriggerKind } from "@maxhq/shared/pills/trigger";
import { deviceName } from "@maxhq/shared/view/select";
import { Maximize2, Minimize2 } from "lucide-react";
import {
	type CSSProperties,
	type ReactNode,
	type Ref,
	useEffect,
	useImperativeHandle,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";

import { useServices, useView } from "@/app/services";
import { cn } from "@/lib/utils";

import { agentDrops, draggedAgentsOf, draggedNoteOf } from "./agent-drop";
import { createAttachments } from "./attachments";
import "./answer-field.css";
import "./composer.css";
import { type EditorChange, PromptEditor, type PromptEditorHandle } from "./editor";
import { timelineFileOf } from "./file-drop";
import { createGitPills, gitFinder, shownItems } from "@maxhq/shared/pills/git-pills";
import { GitPillPreview, useGitHover } from "./git-pills";
import { useComposerNative } from "./native";
import { type PaneFile, usePaneFiles } from "./pane-files";
import { usePillMenu } from "./pill-menu";
import { PILL_CLOCK_MS, PillsContext, type PillsContextValue } from "./pill-nodes";
import { quoteOfDrag } from "./quote-drop";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** How each kind of field sets its text: a card's answer, or the bar at the top. */
const LOOKS = {
	card: { size: 12, line: 18, pad: 4, side: 9 },
	bar: { size: 13, line: 20, pad: 6, side: 11 },
} as const;

/** The tallest a field grows before it scrolls, in pixels. */
export const answerMaxHeight = (look: keyof typeof LOOKS): number =>
	fieldMaxHeight(FIELD_LINES.desktop, LOOKS[look].line, LOOKS[look].pad);

/** Where an answer to `agent` goes, which says which pills it takes. */
export const useAnswerPlace = (agent: AgentId): AnswerPlace => {
	const device = useView((state) => answerPlace(state.view, agent)?.device ?? null);
	const project = useView((state) => answerPlace(state.view, agent)?.project ?? null);
	return useMemo(
		() => (device === null && project === null ? ANY_PLACE : { device, project }),
		[device, project]
	);
};

/** What the box around a field can do to it. */
export type AnswerFieldHandle = {
	readonly focus: () => void;
	readonly blur: () => void;
};

type AnswerFieldProps = {
	readonly ref?: Ref<AnswerFieldHandle>;
	/** What you typed, which the field also takes from outside, as when a send clears it. */
	readonly value: Typed;
	readonly onChange: (typed: Typed) => void;
	/** Enter, without Shift and outside an IME composition. */
	readonly onEnter: () => void;
	readonly label: string;
	readonly placeholder: string;
	/** The agent the answer goes to, whose repo `#` searches, or null for none. */
	readonly agent: AgentId | null;
	/** Where the answer goes, which says which agents and notes it takes. */
	readonly place: AnswerPlace;
	/** The device an agent's pill names while the view doesn't know its workspace. */
	readonly device: DeviceId | null;
	readonly look: keyof typeof LOOKS;
	readonly busy?: boolean;
	/** Set to keep it to one line and cut the text off. */
	readonly shut?: boolean;
	/** The focus came into the field, or left it. */
	readonly onFocus?: () => void;
	readonly onBlur?: () => void;
	/** What sits at the field's end inside its border, such as a send button. */
	readonly trailing?: ReactNode;
	readonly className?: string;
};

/** What you typed, from the text box's change and the pull requests drawn in it. */
const typedFrom = (segments: Segment[], text: string, items: Parameters<typeof shownItems>[0]) =>
	typedOf({ segments, gitItems: shownItems(items, text) });

const sameTyped = (a: Typed, b: Typed): boolean =>
	a.text === b.text && JSON.stringify(a.refs) === JSON.stringify(b.refs);

/** The answer field. */
export const AnswerField = (props: AnswerFieldProps) => {
	useI18n();
	const { ref, value, label, placeholder, agent, place, device, look, busy, shut } = props;
	const services = useServices();
	const native = useComposerNative();
	const paneFiles = usePaneFiles();
	const view = useView((state) => state.view);
	const now = useNow(PILL_CLOCK_MS);
	// An answer holds no files, but the pills' views read their store.
	const [attachments] = useState(createAttachments);
	const [gitPills] = useState(() => {
		const store = createGitPills(
			agent === null ? async () => null : gitFinder(services.source, agent)
		);
		store.getState().seed(keyedGitItems(value.text, draftOf(value).gitItems));
		return store;
	});
	const [initial] = useState(() => draftOf(value).segments);
	const hover = useGitHover();
	const editor = useRef<PromptEditorHandle>(null);
	const frame = useRef<HTMLDivElement>(null);
	/** What the field last told its owner, or was last given. */
	const reported = useRef(value);
	const [change, setChange] = useState<EditorChange>(() => ({
		text: value.text,
		segments: [...initial],
		caret: null,
		held: null,
	}));
	const [note, setNote] = useState<string | null>(null);
	const [tall, setTall] = useState(false);
	const [scrolls, setScrolls] = useState(false);

	useEffect(() => () => gitPills.getState().dispose(), [gitPills]);

	const report = (segments: Segment[], text: string) => {
		const typed = typedFrom(segments, text, gitPills.getState());
		if (sameTyped(typed, reported.current)) return;
		reported.current = typed;
		props.onChange(typed);
	};
	const latest = useRef(report);
	useLayoutEffect(() => {
		latest.current = report;
	});

	// A pull request found after you typed its number goes with the answer.
	useEffect(
		() =>
			gitPills.subscribe(() => {
				const box = editor.current;
				if (box) latest.current(box.segments(), box.text());
			}),
		[gitPills]
	);

	// What you typed changes from outside too, as when a send clears the field.
	useLayoutEffect(() => {
		const box = editor.current;
		if (!box || value.text === reported.current.text) return;
		reported.current = value;
		if (value.text === "") {
			box.reset();
			return;
		}
		const draft = draftOf(value);
		gitPills.getState().seed(keyedGitItems(value.text, draft.gitItems));
		box.setSegments(draft.segments);
		// The value only says what changed, which isn't worth the lint's dependency.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [value.text]);

	const content = () => frame.current?.querySelector<HTMLElement>(".composer-editor") ?? null;

	useEffect(() => {
		content()?.setAttribute("aria-busy", String(busy ?? false));
	}, [busy]);

	// The button that opens the field shows once the text is more than it shows.
	useLayoutEffect(() => {
		const dom = content();
		if (!dom) return;
		const measure = () => setScrolls(dom.scrollHeight > dom.clientHeight + 1);
		measure();
		if (typeof ResizeObserver === "undefined") return;
		const watch = new ResizeObserver(measure);
		watch.observe(dom);
		return () => watch.disconnect();
	}, [change.text, tall, shut]);

	// Shut, it shows the start of the text. Opened or closed, the caret stays in view.
	const opened = useRef(false);
	useLayoutEffect(() => {
		const dom = content();
		if (shut && dom) dom.scrollTop = 0;
	}, [shut]);
	useLayoutEffect(() => {
		if (!opened.current) {
			opened.current = true;
			return;
		}
		editor.current?.focus();
	}, [tall]);

	useImperativeHandle(
		ref,
		() => ({
			focus: () => editor.current?.focus(),
			blur: () => content()?.blur(),
		}),
		[]
	);

	/** An agent's pill, or null while neither the view nor the field knows its device. */
	const pillOf = (other: Agent): AgentRef | null => {
		const now = services.view.getState().view;
		const fallback = place.device ?? device;
		if (fallback === null && !now.tables.workspace.has(other.workspace)) return null;
		return agentPillOf(now, other, fallback ?? "");
	};

	/** Puts a piece in at `at`, with what the field can't take as words and a line why. */
	const take = (
		segments: readonly Segment[],
		at: "caret" | "drop",
		factsOf?: (key: string) => FileFacts | undefined
	) => {
		const box = editor.current;
		if (!box) return;
		const taken = takenPiece(services.view.getState().view, segments, place, factsOf);
		setNote(taken.why);
		if (at === "drop") box.dropSegments(taken.segments);
		else box.insertSegments(taken.segments);
	};

	const noFiles = () => {
		setNote(NO_FILES);
		editor.current?.focus();
	};

	/** Hands files to the pane's composer, or says they can't go in. */
	const handOn = (file: PaneFile) => {
		if (!paneFiles?.send(file)) noFiles();
	};

	useEffect(() => {
		const element = frame.current;
		if (!element) return;
		const quotes = native.registerDropTarget(element, {
			// A file has no place in an answer, so it goes on to the composer.
			onDrop: (paths) => handOn({ type: "paths", paths }),
			takesPageDrag: (drag) => quoteOfDrag(drag) !== null || timelineFileOf(drag) !== null,
			onPageDrop: (drag) => {
				const quote = quoteOfDrag(drag);
				const file = timelineFileOf(drag);
				if (quote) take([{ type: "pill", pill: quote }], "drop");
				else if (file) handOn({ type: "timeline", file });
			},
		});
		const agents = agentDrops.register(element, {
			accepts: (dragged) => {
				const carried = draggedAgentsOf(dragged);
				return carried === null || carried.some((other) => other.id !== agent);
			},
			onHover: () => {},
			onDrop: (dragged) => {
				const note = draggedNoteOf(dragged);
				const others = (draggedAgentsOf(dragged) ?? []).filter(
					(other) => other.id !== agent
				);
				take(
					note
						? [{ type: "pill", pill: note }]
						: others.flatMap((other) => {
								const pill = pillOf(other);
								return pill ? [{ type: "pill" as const, pill }] : [];
							}),
					"drop"
				);
			},
		});
		return () => {
			quotes();
			agents();
		};
		// The handlers read the place and the view as a drop lands.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [agent, native, paneFiles]);

	const paste = (data: DataTransfer): boolean => {
		const fragment = readFragment(data);
		if (fragment) {
			const files = new Map(fragment.files.map((file) => [file.key, file]));
			gitPills
				.getState()
				.seed(keyedGitItems(markedText(fragment.segments), fragment.gitItems));
			take(fragment.segments, "caret", (key) => files.get(key));
			return true;
		}
		if (data.files.length > 0 || Array.from(data.types).includes("Files")) {
			handOn({ type: "pasted", files: Array.from(data.files) });
			return true;
		}
		return false;
	};

	const copy = (segments: Segment[]) => ({
		plain: plainText(segments, () => undefined),
		fragment: encodeFragment({
			version: 1,
			origin: "answer",
			segments,
			files: [],
			gitItems: shownItems(gitPills.getState(), markedText(segments)),
		}),
	});

	const changed = (next: EditorChange) => {
		setChange(next);
		if (next.text !== change.text) setNote(null);
		report(next.segments, next.text);
		if (agent !== null) gitPills.getState().want(gitNumbers(next.text));
	};

	const kinds: readonly TriggerKind[] = agent === null ? ["path"] : ["path", "git-item"];
	const menu = usePillMenu({
		editor,
		text: change.text,
		caret: change.caret,
		agent,
		kinds,
		onPath: null,
		agentsOn: place.device,
		notes: place.project === null ? null : { own: place.project, only: true },
		gitPills,
		agentPill: pillOf,
		below: look === "bar",
	});

	const pills = useMemo(
		(): PillsContextValue => ({
			attachments,
			native,
			numbers: new Map(),
			now,
			agentOf: (id) => view.tables.agent.get(id) ?? null,
			deviceName: (id) => deviceName(view, id),
		}),
		[attachments, native, now, view]
	);

	const { line, pad, side, size } = LOOKS[look];
	const style = {
		"--answer-line": `${line}px`,
		"--answer-pad": `${pad}px`,
		"--answer-side": `${side}px`,
		"--answer-size": `${size}px`,
		"--answer-max": `${answerMaxHeight(look)}px`,
	} as CSSProperties;
	const opens = !shut && (scrolls || tall);

	return (
		<PillsContext value={pills}>
			<div className={cn("flex min-w-0 flex-col gap-1", props.className)}>
				<div
					ref={frame}
					data-state={shut ? "collapsed" : "expanded"}
					data-size={tall && !shut ? "tall" : "fit"}
					style={style}
					className={cn(
						"answer-field relative flex min-w-0 items-stretch border border-input bg-background transition-[border-color,box-shadow] duration-150",
						look === "bar" ? "rounded-[9px]" : "rounded-md",
						look === "bar"
							? "focus-within:border-primary focus-within:shadow-focus data-[state=expanded]:focus-within:shadow-[var(--focus-glow),var(--lift-shadow)]"
							: "focus-within:border-ring"
					)}
					onFocus={(event) => {
						if (!event.currentTarget.contains(event.relatedTarget)) props.onFocus?.();
					}}
					onBlur={(event) => {
						if (event.currentTarget.contains(event.relatedTarget)) return;
						// Clicking away closes the menu, though a click in it keeps the focus.
						menu.close();
						props.onBlur?.();
					}}
				>
					{menu.element}
					<div className="min-w-0 flex-1">
						<PromptEditor
							ref={editor}
							native={native}
							gitPills={gitPills}
							label={label}
							placeholder={placeholder}
							menuAttributes={menu.attributes}
							initial={initial}
							onEnter={() => props.onEnter()}
							onHistoryKey={() => false}
							onAttach={noFiles}
							onPaste={paste}
							onDropFiles={(files) => handOn({ type: "dropped", files })}
							onDropTimelineFile={(file) => handOn({ type: "timeline", file })}
							onChange={changed}
							onCopy={copy}
							onMenuKey={menu.onKey}
							onGitHover={hover.hover}
							onDropQuote={(quote) => take([{ type: "pill", pill: quote }], "drop")}
						/>
					</div>
					{opens ? (
						<button
							type="button"
							aria-label={translate("Show all the text")}
							aria-pressed={tall}
							title={tall ? translate("Show less") : translate("Show all the text")}
							className="mt-[3px] mr-[3px] grid size-6 shrink-0 cursor-pointer place-items-center self-start rounded-[5px] text-muted-foreground outline-none hover:bg-secondary hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
							// The text box keeps the focus, and with it the caret.
							onMouseDown={(event) => event.preventDefault()}
							onClick={() => setTall(!tall)}
						>
							{tall ? (
								<Minimize2 aria-hidden className="size-3.5" strokeWidth={2} />
							) : (
								<Maximize2 aria-hidden className="size-3.5" strokeWidth={2} />
							)}
						</button>
					) : null}
					{props.trailing}
					<GitPillPreview
						store={gitPills}
						shown={hover.shown}
						now={now}
						onHold={hover.hold}
						onClose={hover.close}
					/>
				</div>
				{note !== null ? (
					<p role="alert" className="text-[11.5px] text-status-error-text">
						{note}
					</p>
				) : null}
			</div>
		</PillsContext>
	);
};
