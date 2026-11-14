// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pictures shown together go through one viewer: the ones a message
// attached, the ones a step showed the agent, or the ones a reply shows.
// Each picture joins the set around it, and a click opens the viewer on it
// with the rest in the order they stand on screen. A set of one opens the
// same viewer, without the arrows.

import {
	createContext,
	type ReactNode,
	type RefCallback,
	useCallback,
	useContext,
	useEffect,
	useId,
	useMemo,
	useState,
} from "react";

import { useServices } from "@/app/services";
import { useRow } from "@maxhq/shared/agent-view/context";
import { FULL_LIMIT, loadMedia } from "@maxhq/shared/agent-view/media";
import { pictureNeighbours, stepPicture } from "@maxhq/shared/agent-view/picture-set";

import type { FileDrag } from "./file-drag";
import { type Picture, PictureViewer } from "./picture-viewer";

/** What the viewer shows for one picture. */
export type ShownPicture = {
	readonly name: string;
	readonly picture: Picture;
	readonly drag?: FileDrag;
};

type SetValue = {
	/** The element each picture in the set shows in, by its id. */
	readonly nodes: Map<string, Element>;
	/** What each picture in the set shows, by its id. */
	readonly shown: Map<string, ShownPicture>;
	readonly open: (id: string) => void;
};

const SetContext = createContext<SetValue | null>(null);

// The pictures' ids in the order they stand in the page.
const ordered = (nodes: Map<string, Element>): string[] =>
	[...nodes]
		.filter(([, node]) => node.isConnected)
		.sort(([, a], [, b]) =>
			a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_PRECEDING ? 1 : -1
		)
		.map(([id]) => id);

// Fetches a picture on the device ahead of a step to it.
const PreloadDevice = ({ path }: { path: string }) => {
	const { agent } = useRow();
	const { source } = useServices();
	useEffect(() => {
		void loadMedia(source, agent, path, FULL_LIMIT);
	}, [source, agent, path]);
	return null;
};

const Preload = ({ picture }: { picture: Picture }) => {
	useEffect(() => {
		if (picture.kind === "url") new Image().src = picture.url;
	}, [picture]);
	return picture.kind === "device" ? <PreloadDevice path={picture.path} /> : null;
};

/** Makes the pictures under it one set in the viewer. */
export const PictureSet = ({ children }: { children: ReactNode }) => {
	const [nodes] = useState(() => new Map<string, Element>());
	const [pictures] = useState(() => new Map<string, ShownPicture>());
	const [shown, setShown] = useState<{ list: readonly ShownPicture[]; at: number } | null>(null);
	// Kept apart from what's shown, so the viewer closes the way it opened.
	const [isOpen, setOpen] = useState(false);
	const open = useCallback(
		(id: string) => {
			const ids = ordered(nodes);
			const list = ids.flatMap((each) => pictures.get(each) ?? []);
			setShown({ list, at: Math.max(ids.indexOf(id), 0) });
			setOpen(true);
		},
		[nodes, pictures]
	);
	const set = useMemo(() => ({ nodes, shown: pictures, open }), [nodes, pictures, open]);
	const step = useCallback(
		(delta: number) =>
			setShown((now) =>
				now ? { ...now, at: stepPicture(now.at, now.list.length, delta) } : now
			),
		[]
	);
	const current = shown?.list[shown.at];
	return (
		<SetContext.Provider value={set}>
			{children}
			{shown && current ? (
				<>
					<PictureViewer
						open={isOpen}
						onOpenChange={setOpen}
						name={current.name}
						picture={current.picture}
						drag={current.drag}
						nav={
							shown.list.length > 1
								? { at: shown.at, count: shown.list.length, step }
								: undefined
						}
					/>
					{isOpen
						? pictureNeighbours(shown.at, shown.list.length).map((i) => (
								<Preload key={i} picture={shown.list[i]!.picture} />
							))
						: null}
				</>
			) : null}
		</SetContext.Provider>
	);
};

/**
 * Joins the set around a picture, if there is one. Gives what to pass as
 * the ref of the element it shows in, which places it in the set, and what
 * opens the viewer on it, or null outside a set, where the picture keeps a
 * viewer of its own.
 */
export const useInPictureSet = (
	shown: ShownPicture
): { place: RefCallback<Element> | undefined; open: (() => void) | null } => {
	const set = useContext(SetContext);
	const id = useId();
	useEffect(() => {
		if (!set) return;
		set.shown.set(id, shown);
		return () => {
			set.shown.delete(id);
		};
	});
	const place = useCallback<RefCallback<Element>>(
		(node) => {
			if (!set) return;
			if (node) set.nodes.set(id, node);
			else set.nodes.delete(id);
		},
		[set, id]
	);
	const open = useCallback(() => set?.open(id), [set, id]);
	return set ? { place, open } : { place: undefined, open: null };
};
