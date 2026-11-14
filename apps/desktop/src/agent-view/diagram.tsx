// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A Mermaid diagram in a message: a picture with its source a click away. It
// shows its source while the text is still streaming in, and draws once it's
// near the visible part of the window. Its height is known before the picture
// decodes, so the feed doesn't shift. Find and quotes leave it out.

import { useEffect, useReducer, useState } from "react";

import { useTheme } from "@/app/theme";
import { CopyButton } from "@/components/copy-button";
import { cn } from "@/lib/utils";

import { CodeBlock } from "./code-block";
import { allowDiagram, drawDiagram, drawnDiagram, type Drawing } from "./diagram-render";
import { PictureViewer, useSeen } from "./picture-viewer";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const NAME = "Mermaid diagram";

/** The tallest a diagram shows in the feed, which a click shows whole. */
const TALLEST_PX = 560;

/** The same in the text box, which is itself only so tall. */
const PREVIEW_TALLEST_PX = 240;

const HEADER_BUTTON =
	"h-[22px] cursor-pointer rounded-[5px] px-1.5 font-sans text-[11px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring";

const useDrawing = (source: string, wanted: boolean) => {
	const [, drew] = useReducer((count: number) => count + 1, 0);
	// A new theme draws the diagram again in its colours, even mid-drawing.
	const theme = useTheme();
	const drawn = drawnDiagram(source);
	useEffect(() => {
		if (!wanted || drawn) return;
		let current = true;
		void drawDiagram(source).then(() => {
			if (current) drew();
		});
		return () => {
			current = false;
		};
	}, [source, wanted, drawn, theme]);
	const drawAnyway = () => {
		allowDiagram(source);
		drew();
	};
	return { drawing: drawn, drawAnyway };
};

/** A drawn diagram's picture at its own size, or smaller to fit. */
export const DiagramPicture = ({
	drawing,
	source,
	tallest = TALLEST_PX,
}: {
	drawing: Extract<Drawing, { state: "drawn" }>;
	source: string;
	tallest?: number;
}) => {
	useI18n();
	const { url, width, height } = drawing;
	const most = Math.min(width, (tallest * width) / height);
	return (
		<img
			src={url}
			alt={source}
			draggable={false}
			className="mx-auto block"
			style={{ width: "100%", maxWidth: most, aspectRatio: `${width} / ${height}` }}
		/>
	);
};

/** The block as code, when it's still coming in or won't draw. */
const DiagramSource = ({
	text,
	live = false,
	failure,
	drawAnyway,
	className,
}: {
	text: string;
	live?: boolean;
	failure?: Extract<Drawing, { state: "failed" }> | undefined;
	drawAnyway?: () => void;
	className?: string | undefined;
}) => {
	useI18n();
	return (
		<div data-find-skip className={cn("flex flex-col gap-1", className)}>
			<CodeBlock code={text} lang="mermaid" live={live} />
			{failure === undefined ? null : (
				<div className="flex items-center gap-1.5">
					<p className="min-w-0 text-[11.5px] text-muted-foreground">
						{translate("Couldn't draw this diagram. ")}
						{failure.reason}
					</p>
					{failure.heldBack ? (
						<button
							type="button"
							className={cn(HEADER_BUTTON, "shrink-0")}
							onClick={drawAnyway}
						>
							{translate("Draw anyway")}
						</button>
					) : null}
				</div>
			)}
		</div>
	);
};

/** How a diagram reads while its message is still being written. */
export const DiagramPreview = ({ source }: { source: string }) => {
	useI18n();
	const { drawing, drawAnyway } = useDrawing(source, true);
	if (drawing?.state === "failed")
		return <DiagramSource text={source} failure={drawing} drawAnyway={drawAnyway} />;
	return (
		<div className="rounded-lg border border-border bg-subpanel p-3">
			{drawing ? (
				<DiagramPicture drawing={drawing} source={source} tallest={PREVIEW_TALLEST_PX} />
			) : (
				<div
					className="h-24 animate-pulse"
					role="status"
					aria-label={translate("Drawing the diagram")}
				/>
			)}
		</div>
	);
};

/** A fenced `mermaid` block. */
export const Diagram = ({
	source,
	live = false,
	className,
}: {
	source: string;
	/** Set while the block is still streaming in. */
	live?: boolean;
	className?: string;
}) => {
	useI18n();
	const text = source.replace(/\n$/, "");
	const [ref, seen] = useSeen<HTMLDivElement>();
	const { drawing, drawAnyway } = useDrawing(text, seen && !live);
	const [showSource, setShowSource] = useState(false);
	const [whole, setWhole] = useState<string | null>(null);

	if (live || drawing?.state === "failed")
		return (
			<DiagramSource
				text={text}
				live={live}
				failure={drawing?.state === "failed" && !live ? drawing : undefined}
				drawAnyway={drawAnyway}
				className={className}
			/>
		);
	return (
		<div
			ref={ref}
			data-find-skip
			className={cn(
				"overflow-hidden rounded-lg border border-border bg-subpanel select-none",
				className
			)}
		>
			<div className="flex h-[30px] items-center gap-1 border-b border-border pr-1.5 pl-3 font-mono text-[11.5px] text-muted-foreground">
				<span className="min-w-0 flex-1 truncate">mermaid</span>
				<button
					type="button"
					aria-pressed={showSource}
					className={HEADER_BUTTON}
					onClick={() => setShowSource((shown) => !shown)}
				>
					{showSource ? translate("Diagram") : translate("Source")}
				</button>
				<CopyButton text={text} />
			</div>
			{showSource ? (
				<pre className="overflow-x-auto px-3 py-2.5 font-mono text-[11.5px] leading-[1.6] text-terminal select-text">
					<code>{text}</code>
				</pre>
			) : drawing?.state === "drawn" ? (
				<button
					type="button"
					aria-label={translate("Show the {{value1}} whole", {
						value1: NAME.toLowerCase(),
					})}
					className="block w-full cursor-zoom-in p-3 outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
					onClick={() => setWhole(drawing.whole())}
				>
					<DiagramPicture drawing={drawing} source={text} />
				</button>
			) : (
				<div
					className="h-24 animate-pulse"
					role="status"
					aria-label={translate("Drawing the diagram")}
				/>
			)}
			{whole === null ? null : (
				<PictureViewer
					open
					onOpenChange={(open) => {
						if (!open) setWhole(null);
					}}
					name={NAME}
					picture={{ kind: "url", url: whole }}
				/>
			)}
		</div>
	);
};
