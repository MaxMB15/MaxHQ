// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: opening a
// workspace, or a file in it at a line, in an editor on this Mac. The header's
// split button opens the editor you used last and its chevron lists the
// others, then Terminal, which opens the agent's shell in the terminal
// panel; with no editor the button is a menu with only Terminal. A menu
// lists them all, a sidebar row's menu offers the one the button uses,
// and a file row in the timeline opens that file. When the
// workspace can't open in an editor, the items say why. A file the viewer
// shows, such as a picture or a 3D model, opens in a file pane in MaxHQ
// instead, and the row's chevron still lists the editors. Any pick becomes the
// last editor used, kept in localStorage for every window, and the buttons use
// it unless the default editor setting fixes them on one or hides them. A
// container goes over SSH to MaxHQ's alias for it, so until your
// `~/.ssh/config` includes MaxHQ's, a pick first asks whether the app may add
// that one line.

import { Dialog } from "@base-ui/react/dialog";
import { Menu } from "@base-ui/react/menu";
import { skipToken, useQuery, useQueryClient } from "@tanstack/react-query";
import {
	ChevronDown,
	CircleAlert,
	Ellipsis,
	Eye,
	SquareArrowOutUpRight,
	SquareTerminal,
} from "lucide-react";
import { useMemo, useState } from "react";
import { useStore } from "zustand";

import { useServices, useView } from "@/app/services";
import { panesOf } from "@/layout/panes";
import { keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { useTerminalActions } from "@/terminal";
import { CopyButton } from "@/components/copy-button";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { PRIMARY, SECONDARY } from "@/settings/controls";
import { BACKDROP, POPUP, Title } from "@/workspaces/new-workspace";
import type { AgentId, SshIncludeState, WorkspaceId } from "@maxhq/shared/bindings";
import type { Editors } from "@maxhq/shared/data/source";
import { type EditorReach, editorReach, preferredEditor } from "@maxhq/shared/editor/open";
import { cn } from "@/lib/utils";
import { runPlace } from "@maxhq/shared/terminal/shell";
import { errorText } from "@maxhq/shared/timeline/store";
import { linkForDevice } from "@maxhq/shared/view/select";

import { useRow } from "@maxhq/shared/agent-view/context";
import { editorPrefsOf, setLast } from "./editor-prefs";
import { MENU_KEY } from "./run-menu";
import { nameOf } from "@maxhq/shared/folders/path";
import { viewerKind } from "@maxhq/shared/agent-view/viewer-files";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** One editor this Mac has. */
export type EditorChoice = Editors["available"][number];

/** This Mac's editors and every one the app knows, or undefined until the app says. */
export const useEditors = (): Editors | undefined => {
	const { source } = useServices();
	// This Mac's editors only change when you install one, so one look per window.
	return useQuery({
		queryKey: ["editors"],
		queryFn: () => source.listEditors(),
		staleTime: Infinity,
	}).data;
};

/** A file to open: its path as the agent named it, and a line in it. */
export type OpenAt = { readonly path: string; readonly line?: number };

/** What the Open in controls need for one workspace. */
export type OpenIn = {
	readonly editors: readonly EditorChoice[];
	/** The editor the buttons use, or null when the default editor is None. */
	readonly preferred: EditorChoice | null;
	readonly reach: EditorReach;
	/** Opens the workspace, or a file in it, and resolves to why it couldn't, or null. */
	readonly open: (editor: EditorChoice, at?: OpenAt) => Promise<string | null>;
	/**
	 * Adds the `Include` line to your `~/.ssh/config`, or with `add` false
	 * records that you'll add the entry yourself, and resolves to where the
	 * container stands after.
	 */
	readonly setupSsh: (add: boolean) => Promise<SshIncludeState>;
};

/** The first part of every SSH include query's key. */
const SSH_INCLUDE = "ssh-include";

/** The Open in controls' state, or null while there's no workspace or no editor. */
export const useOpenIn = (workspace: WorkspaceId | undefined): OpenIn | null => {
	const { t: translate } = useI18n();
	const services = useServices();
	const { source } = services;
	const editors = useEditors()?.available;
	const found = useView((s) => (workspace ? s.view.tables.workspace.get(workspace) : undefined));
	const link = useView((s) => (found ? linkForDevice(s.view, found.device) : undefined));
	const { last, fixed } = useStore(editorPrefsOf(services));
	// A container's alias comes once it runs and its guest reports a host
	// key, so the state is asked again when either changes.
	const queries = useQueryClient();
	const container = found?.location.type === "container" ? found : undefined;
	const includeKey = useMemo(
		() => [
			SSH_INCLUDE,
			container?.id,
			container?.managed?.state.state ?? null,
			container?.managed?.ssh_host_key ?? null,
		],
		[container]
	);
	const { data: answered, error: unchecked } = useQuery({
		queryKey: includeKey,
		queryFn: container ? () => source.sshInclude(container.id) : skipToken,
	});
	const include = useMemo<SshIncludeState | undefined>(
		() =>
			unchecked
				? {
						type: "unavailable",
						reason: errorMessage(unchecked, translate("Couldn't check the SSH setup")),
					}
				: answered,
		[answered, unchecked, translate]
	);

	return useMemo(() => {
		if (!found || !editors || editors.length === 0) return null;
		const preferred = preferredEditor(editors, last, fixed);
		const open = async (editor: EditorChoice, at?: OpenAt): Promise<string | null> => {
			setLast(services, editor.id);
			try {
				const result = await source.openInEditor(editor.id, found.id, at?.path, at?.line);
				return result.status === "ok" ? null : errorText(result.error);
			} catch (error) {
				return errorMessage(
					error,
					translate("Couldn't reach {{label}}.", { label: editor.label })
				);
			}
		};
		// The answer goes into this query at once, and every other workspace
		// asks again, since they share one `~/.ssh/config`.
		const setupSsh = async (add: boolean): Promise<SshIncludeState> => {
			const state = await source.setupSshInclude(found.id, add);
			queries.setQueryData(includeKey, state);
			void queries.invalidateQueries({ queryKey: [SSH_INCLUDE] });
			return state;
		};
		return {
			editors,
			preferred,
			reach: editorReach(found, link, include),
			open,
			setupSsh,
		};
	}, [
		services,
		source,
		queries,
		editors,
		found,
		link,
		last,
		fixed,
		include,
		includeKey,
		translate,
	]);
};

const errorMessage = (error: unknown, fallback: string): string =>
	error instanceof Error && error.message ? error.message : fallback;

/** Why the workspace can't open in an editor, or null when it can. */
const blocked = (reach: EditorReach): string | null =>
	reach.kind === "unavailable" ? reach.reason : null;

/** What an item says under its label about how the editor gets there. */
const reachDetail = (reach: EditorReach): string | null => {
	switch (reach.kind) {
		case "local":
			return null;
		case "remote":
			return translate("Over SSH to {{host}}", { host: reach.host });
		case "wsl":
			return translate("Through WSL in {{distro}}", { distro: reach.distro });
		case "ask":
			return translate("Over SSH to {{host}}, after one line of setup", { host: reach.host });
		case "declined":
			return translate("Over SSH to {{host}}, with your own SSH entry", { host: reach.host });
		case "unavailable":
			return reach.reason;
	}
};

const LABEL = "text-[12.5px] font-[550]";
const DETAIL = "text-[11px] text-muted-foreground";
const MONO =
	"max-h-[200px] overflow-auto rounded-[7px] border border-border bg-subpanel px-2.5 py-2 font-mono text-xs whitespace-pre text-foreground select-text";

/** An editor someone picked, and the file to open in it, if any. */
type Pick = { readonly editor: EditorChoice; readonly at?: OpenAt };

/** Says why `editor` didn't open. */
type Report = (editor: EditorChoice, failure: string) => void;

/**
 * The SSH setup a container needs before an editor reaches it. While the
 * app may still ask, it says what line it adds. Once you've said no, it
 * shows the entry to copy, and can still add the line.
 */
const SshSetupDialog = ({
	openIn,
	pick,
	onClose,
	report,
}: {
	openIn: OpenIn;
	pick: Pick;
	onClose: () => void;
	report: Report;
}) => {
	useI18n();
	const [busy, setBusy] = useState<"add" | "decline" | null>(null);
	const [failure, setFailure] = useState<string | null>(null);
	const { reach } = openIn;
	const { editor, at } = pick;
	if (reach.kind !== "ask" && reach.kind !== "declined") return null;

	const openNow = () => {
		onClose();
		void openIn.open(editor, at).then((why) => {
			if (why) report(editor, why);
		});
	};
	const setup = async (add: boolean) => {
		setBusy(add ? "add" : "decline");
		setFailure(null);
		try {
			const state = await openIn.setupSsh(add);
			if (add && state.type === "included") openNow();
			else if (state.type === "unavailable") {
				onClose();
				report(editor, state.reason);
			} else if (add)
				setFailure(translate("The line didn't take. Your ~/.ssh/config is as it was."));
		} catch (error) {
			setFailure(errorMessage(error, translate("Couldn't change your SSH setup.")));
		} finally {
			setBusy(null);
		}
	};

	return (
		<div className="flex flex-col gap-3.5">
			{reach.kind === "ask" ? (
				<>
					<Title>{translate("Let editors reach this container")}</Title>
					<Dialog.Description className="text-[12.5px] text-muted-foreground">
						{translate(
							"MaxHQ adds one line at the top of your ~/.ssh/config so editors can reach its containers, such as {{host}}. It changes nothing else.",
							{ host: reach.host }
						)}
					</Dialog.Description>
					<pre aria-label={translate("The line MaxHQ adds")} className={MONO}>
						{reach.line}
					</pre>
				</>
			) : (
				<>
					<Title>{translate("Add this container to your SSH config")}</Title>
					<Dialog.Description className="text-[12.5px] text-muted-foreground">
						{translate(
							"Copy this entry into your ~/.ssh/config so {{label}} can reach {{host}}, then open it. Or let MaxHQ add its one line after all.",
							{ label: editor.label, host: reach.host }
						)}
					</Dialog.Description>
					<pre aria-label={translate("The entry to copy")} className={MONO}>
						{reach.block}
					</pre>
					<div className="flex justify-end">
						<CopyButton text={reach.block} className={SECONDARY} />
					</div>
				</>
			)}
			{failure ? (
				<p role="alert" className="text-[11.5px] text-status-error-text">
					{failure}
				</p>
			) : null}
			<div className="flex items-center justify-end gap-2">
				<Dialog.Close className={SECONDARY}>{translate("Cancel")}</Dialog.Close>
				{reach.kind === "ask" ? (
					<>
						<button
							type="button"
							className={SECONDARY}
							disabled={busy !== null}
							onClick={() => void setup(false)}
						>
							{translate("I'll add it myself")}
						</button>
						<button
							type="button"
							className={PRIMARY}
							disabled={busy !== null}
							onClick={() => void setup(true)}
						>
							{busy === "add"
								? translate("Adding")
								: translate("Add the line and open")}
						</button>
					</>
				) : (
					<>
						<button
							type="button"
							className={SECONDARY}
							disabled={busy !== null}
							onClick={() => void setup(true)}
						>
							{busy === "add"
								? translate("Adding")
								: translate("Let MaxHQ add the line")}
						</button>
						<button type="button" className={PRIMARY} onClick={openNow}>
							{translate("Open in {{label}}", { label: editor.label })}
						</button>
					</>
				)}
			</div>
		</div>
	);
};

/** Whether a pick first asks about the SSH setup. */
const asks = (reach: EditorReach): boolean => reach.kind === "ask" || reach.kind === "declined";

/** The SSH setup question for a pick, open while the container still needs it. */
const SshSetup = ({
	openIn,
	pick,
	onClose,
	report,
}: {
	openIn: OpenIn;
	pick: Pick;
	onClose: () => void;
	report: Report;
}) => {
	useI18n();
	return (
		<Dialog.Root
			open={asks(openIn.reach)}
			onOpenChange={(next) => {
				if (!next) onClose();
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={POPUP}>
					<SshSetupDialog openIn={openIn} pick={pick} onClose={onClose} report={report} />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};

/**
 * Opens a pick in its editor, or first asks about the SSH setup when a
 * container still needs it. `dialog` is that question, to render anywhere.
 */
const usePick = (openIn: OpenIn | null, report: Report) => {
	const [pick, setPick] = useState<Pick | null>(null);
	const choose = (editor: EditorChoice, at?: OpenAt) => {
		if (!openIn) return;
		if (asks(openIn.reach)) {
			setPick({ editor, at });
			return;
		}
		void openIn.open(editor, at).then((failure) => {
			if (failure) report(editor, failure);
		});
	};
	const dialog =
		openIn && pick ? (
			<SshSetup openIn={openIn} pick={pick} onClose={() => setPick(null)} report={report} />
		) : null;
	return { choose, dialog };
};

/** Menu items that open the workspace in each of `editors`. */
const EditorItems = ({
	openIn,
	editors,
	choose,
}: {
	openIn: OpenIn;
	editors: readonly EditorChoice[];
	choose: (editor: EditorChoice) => void;
}) => {
	useI18n();
	const why = blocked(openIn.reach);
	const detail = reachDetail(openIn.reach);
	return editors.map((editor) => (
		<Menu.Item
			key={editor.id}
			disabled={why !== null}
			onClick={() => choose(editor)}
			className={MENU_ITEM}
		>
			<SquareArrowOutUpRight
				aria-hidden
				size={13}
				strokeWidth={2}
				className="shrink-0 text-secondary-foreground"
			/>
			<span className="flex min-w-0 flex-1 flex-col gap-px">
				<span className={LABEL}>
					{translate("Open in {{label}}", { label: editor.label })}
				</span>
				{detail ? <span className={cn(DETAIL, "truncate")}>{detail}</span> : null}
			</span>
		</Menu.Item>
	));
};

const POSITIONER = { side: "bottom", align: "end", sideOffset: 6, className: "z-[130]" } as const;

/** The item that opens the agent's shell in the terminal panel. */
const TerminalItem = ({ agent }: { agent: AgentId }) => {
	useI18n();
	const actions = useTerminalActions();
	const view = useView((s) => s.view);
	const place = runPlace(view, agent);
	return (
		<Menu.Item
			label={translate("Terminal")}
			onClick={() => actions.openAgentShell(agent)}
			className={MENU_ITEM}
		>
			<SquareTerminal
				aria-hidden
				size={13}
				strokeWidth={2}
				className="shrink-0 text-secondary-foreground"
			/>
			<span className="flex min-w-0 flex-1 flex-col gap-px">
				<span className={LABEL}>{translate("Terminal")}</span>
				{place ? (
					<span className={cn(DETAIL, "truncate")}>
						{translate("The agent's shell, in {{folder}} on {{device}}", {
							folder: place.folder,
							device: place.deviceName,
						})}
					</span>
				) : null}
			</span>
			<kbd aria-hidden className={MENU_KEY}>
				{keyHint(SHORTCUTS.terminal)}
			</kbd>
		</Menu.Item>
	);
};

const SPLIT =
	"inline-flex h-[22px] shrink-0 items-stretch overflow-hidden rounded-md border border-track bg-secondary text-[11.5px] text-secondary-foreground";
const PART =
	"inline-flex items-center outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset";

/** Open in ▾ with only Terminal, for when there's no editor to open. */
const TerminalOnly = ({ agent, labelClassName }: { agent: AgentId; labelClassName?: string }) => {
	useI18n();
	return (
		<Menu.Root>
			<Menu.Trigger
				aria-label={translate("Open in")}
				className={cn(
					SPLIT,
					PART,
					"cursor-pointer gap-[5px] px-[7px] data-popup-open:bg-border data-popup-open:text-foreground"
				)}
			>
				<SquareArrowOutUpRight aria-hidden size={11} strokeWidth={2} />
				<span className={labelClassName}>{translate("Open in")}</span>
				<ChevronDown aria-hidden size={11} strokeWidth={2.2} />
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner {...POSITIONER}>
					<Menu.Popup className={cn(MENU_POPUP, "w-[260px]")}>
						<TerminalItem agent={agent} />
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};

/**
 * The header's split button: the main part opens the workspace, or the file
 * `at` names in it, in the default editor or the last one used, and the
 * chevron lists the others, then Terminal when `terminal` names the agent
 * whose shell it opens. With no editor, or the default None, it's a menu
 * with only Terminal, or nothing without `terminal`.
 */
export const OpenInButton = ({
	workspace,
	at,
	onFail,
	labelClassName,
	terminal,
}: {
	workspace: WorkspaceId;
	at?: OpenAt;
	onFail: (text: string) => void;
	/** Classes for the button's words, such as one that hides them where it's narrow. */
	labelClassName?: string;
	/** The agent whose shell Terminal opens, while its device has terminals. */
	terminal?: AgentId;
}) => {
	useI18n();
	const openIn = useOpenIn(workspace);
	const { choose, dialog } = usePick(openIn, (editor, failure) =>
		onFail(
			translate("Couldn't open {{label}}. {{failure}}", {
				label: editor.label,
				failure: failure,
			})
		)
	);
	if (!openIn?.preferred)
		return terminal ? <TerminalOnly agent={terminal} labelClassName={labelClassName} /> : null;
	const { preferred } = openIn;
	const why = blocked(openIn.reach);
	const others = openIn.editors.filter((e) => e.id !== preferred.id);

	return (
		<div role="group" aria-label={translate("Open in editor")} className={SPLIT}>
			<button
				type="button"
				disabled={why !== null}
				title={
					why ??
					translate("Open the {{value1}} in {{label}}", {
						value1: at ? "file" : "folder",
						label: preferred.label,
					})
				}
				onClick={() => choose(preferred, at)}
				className={cn(
					PART,
					"cursor-pointer gap-[5px] px-[7px] disabled:cursor-default disabled:opacity-60 disabled:hover:bg-transparent disabled:hover:text-secondary-foreground"
				)}
			>
				<SquareArrowOutUpRight aria-hidden size={11} strokeWidth={2} />
				<span className={labelClassName}>
					{translate("Open in {{label}}", { label: preferred.label })}
				</span>
			</button>
			{others.length > 0 || terminal ? (
				<>
					<span aria-hidden className="w-px bg-track" />
					<Menu.Root>
						<Menu.Trigger
							aria-label={translate("Choose editor")}
							className={cn(
								PART,
								"justify-center px-1 data-popup-open:bg-border data-popup-open:text-foreground"
							)}
						>
							<ChevronDown aria-hidden size={11} strokeWidth={2.2} />
						</Menu.Trigger>
						<Menu.Portal>
							<Menu.Positioner {...POSITIONER}>
								<Menu.Popup className={cn(MENU_POPUP, "w-[240px]")}>
									<EditorItems
										openIn={openIn}
										editors={others}
										choose={(editor) => choose(editor, at)}
									/>
									{terminal && others.length > 0 ? (
										<Menu.Separator className="mx-1 my-[5px] h-px bg-border" />
									) : null}
									{terminal ? <TerminalItem agent={terminal} /> : null}
								</Menu.Popup>
							</Menu.Positioner>
						</Menu.Portal>
					</Menu.Root>
				</>
			) : null}
			{dialog}
		</div>
	);
};

/** A More menu with every editor, for a workspace card. */
export const OpenInMenu = ({
	workspace,
	name,
	onFail,
}: {
	workspace: WorkspaceId;
	/** The workspace's name, for the button's label. */
	name: string;
	onFail: (text: string) => void;
}) => {
	useI18n();
	const openIn = useOpenIn(workspace);
	const { choose, dialog } = usePick(openIn, (editor, failure) =>
		onFail(
			translate("Couldn't open {{label}}. {{failure}}", {
				label: editor.label,
				failure: failure,
			})
		)
	);
	if (!openIn) return null;
	return (
		<>
			<Menu.Root>
				<Menu.Trigger
					aria-label={translate("More for {{name}}", { name: name })}
					title={translate("More")}
					className="grid size-6 shrink-0 place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-border data-popup-open:text-foreground"
				>
					<Ellipsis aria-hidden size={14} strokeWidth={2.2} />
				</Menu.Trigger>
				<Menu.Portal>
					<Menu.Positioner {...POSITIONER}>
						<Menu.Popup className={cn(MENU_POPUP, "w-[260px]")}>
							<EditorItems openIn={openIn} editors={openIn.editors} choose={choose} />
						</Menu.Popup>
					</Menu.Positioner>
				</Menu.Portal>
			</Menu.Root>
			{dialog}
		</>
	);
};

/**
 * The item that opens the workspace in the default editor or the last one
 * used, for a menu that's gone before the SSH question could show, such as
 * a sidebar row's. `onAsk` hands that question to an `OpenInSetup` outside
 * the menu. Renders nothing when this Mac has no editor or the default is
 * None.
 */
export const OpenInItem = ({
	workspace,
	onAsk,
	onFail,
}: {
	workspace: WorkspaceId;
	onAsk: (editor: EditorChoice) => void;
	onFail: (text: string) => void;
}) => {
	useI18n();
	const openIn = useOpenIn(workspace);
	if (!openIn?.preferred) return null;
	const choose = (editor: EditorChoice) => {
		if (asks(openIn.reach)) onAsk(editor);
		else
			void openIn.open(editor).then((failure) => {
				if (failure)
					onFail(
						translate("Couldn't open {{label}}. {{failure}}", {
							label: editor.label,
							failure: failure,
						})
					);
			});
	};
	return <EditorItems openIn={openIn} editors={[openIn.preferred]} choose={choose} />;
};

/** The SSH question an `OpenInItem` handed on, which opens the editor once answered. */
export const OpenInSetup = ({
	workspace,
	editor,
	onClose,
	onFail,
}: {
	workspace: WorkspaceId;
	editor: EditorChoice;
	onClose: () => void;
	onFail: (text: string) => void;
}) => {
	useI18n();
	const openIn = useOpenIn(workspace);
	if (!openIn) return null;
	return (
		<SshSetup
			openIn={openIn}
			pick={{ editor }}
			onClose={onClose}
			report={(chosen, failure) =>
				onFail(
					translate("Couldn't open {{label}}. {{failure}}", {
						label: chosen.label,
						failure: failure,
					})
				)
			}
		/>
	);
};

/** A small square button at the end of a timeline row. */
export const ROW_BUTTON =
	"inline-flex size-[22px] shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground";

/**
 * A timeline file row's buttons for a file the viewer shows: one that opens
 * it in a file pane, and a chevron that lists the editors, for when you
 * want its source. The chevron is left out when the workspace can't open
 * in an editor.
 */
const ShowFileButton = ({ path, line }: { path: string; line?: number }) => {
	useI18n();
	const services = useServices();
	const { agent } = useRow();
	const workspace = useView((s) => s.view.tables.agent.get(agent)?.workspace);
	const openIn = useOpenIn(workspace);
	const [failure, setFailure] = useState<string | null>(null);
	const { choose, dialog } = usePick(openIn, (_editor, why) => setFailure(why));
	const name = nameOf(path);
	const editors = openIn && !blocked(openIn.reach) ? openIn.editors : [];
	return (
		<>
			{failure ? (
				<span role="alert" className="sr-only">
					{failure}
				</span>
			) : null}
			<button
				type="button"
				aria-label={translate("Show {{name}}", { name: name })}
				title={translate("Show in MaxHQ")}
				onClick={() => {
					setFailure(null);
					panesOf(services).openFile(agent, path, { line });
				}}
				className={ROW_BUTTON}
			>
				<Eye size={12} strokeWidth={1.9} aria-hidden />
			</button>
			{openIn && editors.length > 0 ? (
				<Menu.Root>
					<Menu.Trigger
						aria-label={translate("Open {{name}} in an editor", { name: name })}
						title={failure ?? translate("Open in an editor")}
						className={cn(
							ROW_BUTTON,
							"w-4 data-popup-open:bg-accent data-popup-open:text-foreground",
							failure && "text-status-error-text"
						)}
					>
						{failure ? (
							<CircleAlert size={11} strokeWidth={2} aria-hidden />
						) : (
							<ChevronDown size={11} strokeWidth={2.2} aria-hidden />
						)}
					</Menu.Trigger>
					<Menu.Portal>
						<Menu.Positioner {...POSITIONER}>
							<Menu.Popup className={cn(MENU_POPUP, "w-[240px]")}>
								<EditorItems
									openIn={openIn}
									editors={editors}
									choose={(editor) => {
										setFailure(null);
										choose(editor, { path, line });
									}}
								/>
							</Menu.Popup>
						</Menu.Positioner>
					</Menu.Portal>
				</Menu.Root>
			) : null}
			{dialog}
		</>
	);
};

/**
 * A timeline file row's button that opens the file at `line` in the editor
 * the header's button uses, or in a file pane when it's a file the viewer
 * shows. The editor's button renders nothing when the agent's workspace
 * can't open in one or the default editor is None, so rows stay as they
 * are. A failure turns it red with the reason.
 */
export const OpenFileButton = ({ path, line }: { path: string; line?: number }) => {
	useI18n();
	return viewerKind(path) ? (
		<ShowFileButton path={path} line={line} />
	) : (
		<EditFileButton path={path} line={line} />
	);
};

const EditFileButton = ({ path, line }: { path: string; line?: number }) => {
	useI18n();
	const { agent } = useRow();
	const workspace = useView((s) => s.view.tables.agent.get(agent)?.workspace);
	const openIn = useOpenIn(workspace);
	const [failure, setFailure] = useState<string | null>(null);
	const { choose, dialog } = usePick(openIn, (_editor, why) => setFailure(why));
	if (!openIn?.preferred || blocked(openIn.reach)) return null;
	const { preferred } = openIn;
	const label = translate("Open {{baseName}} in {{label}}", {
		baseName: nameOf(path),
		label: preferred.label,
	});

	return (
		<>
			{failure ? (
				<span role="alert" className="sr-only">
					{failure}
				</span>
			) : null}
			<button
				type="button"
				aria-label={label}
				title={failure ?? translate("Open in {{label}}", { label: preferred.label })}
				onClick={() => {
					setFailure(null);
					choose(preferred, { path, line });
				}}
				className={cn(ROW_BUTTON, failure && "text-status-error-text")}
			>
				{failure ? (
					<CircleAlert size={12} strokeWidth={2} aria-hidden />
				) : (
					<SquareArrowOutUpRight size={12} strokeWidth={1.9} aria-hidden />
				)}
			</button>
			{dialog}
		</>
	);
};
