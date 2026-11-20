// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A picture you can zoom and pan, as in Preview. A pinch or a ⌘-scroll
// zooms around the pointer, a plain scroll or a drag pans while the
// picture is bigger than its frame, and a double click goes between fitted
// and the picture's own pixels. + and - zoom, 0 fits and 1 shows actual
// size. A small control in the corner does the same. The picture moves by
// one CSS transform written straight to it, so a pinch renders nothing.

import { Scan, ZoomIn, ZoomOut } from "lucide-react";
import {
	type KeyboardEvent as ReactKeyboardEvent,
	type PointerEvent as ReactPointerEvent,
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react";

import { cn } from "@/lib/utils";
import { formatNumber, t as translate, useI18n } from "@maxhq/shared/i18n";
import {
	fitted,
	isFitted,
	panBy,
	pannable,
	placed,
	type Point,
	type Size,
	toggleActual,
	wheelFactor,
	type Zoom,
	type ZoomAction,
	zoomKey,
	ZOOM_STEP,
	zoomAt,
} from "@maxhq/shared/agent-view/zoom";

import type { FileDrag } from "./file-drag";

// Safari's pinch, which WebKit sends instead of a wheel with Control held.
type GestureEvent = Event & {
	readonly scale: number;
	readonly clientX: number;
	readonly clientY: number;
};

const CONTROL_BUTTON =
	"grid h-6 min-w-6 cursor-pointer place-items-center rounded-[5px] px-1 hover:bg-media-foreground/15";

// How far a press may move and still count as a click.
const CLICK_SLOP = 3;

/**
 * A picture that fills `className`'s frame, fitted at first. `keys` says
 * where its keys come from: the whole window, for a viewer over it, or the
 * frame once it has focus. A click on the frame around the picture calls
 * `onBackground`. `drag` lets the picture drag out as its file while it
 * fits, since a drag pans once it's bigger.
 */
export const ZoomPicture = ({
	src,
	alt,
	onError,
	drag,
	keys,
	onBackground,
	className,
}: {
	src: string;
	alt: string;
	onError?: () => void;
	drag?: FileDrag;
	keys: "window" | "focus";
	onBackground?: () => void;
	className?: string;
}) => {
	useI18n();
	const frame = useRef<HTMLDivElement>(null);
	const image = useRef<HTMLImageElement>(null);
	const zoom = useRef<Zoom>({ scale: 1, x: 0, y: 0 });
	const picture = useRef<Size | null>(null);
	const box = useRef<Size>({ width: 0, height: 0 });
	// Whether the picture follows the frame's size, as it does until you zoom.
	const following = useRef(true);
	const press = useRef<{ x: number; y: number; moved: boolean } | null>(null);
	const [level, setLevel] = useState<number | null>(null);
	const [canPan, setCanPan] = useState(false);
	const [dragging, setDragging] = useState(false);

	const show = useCallback((next: Zoom, animate = false) => {
		const size = picture.current;
		const img = image.current;
		if (!size || !img) return;
		zoom.current = next;
		img.style.transition = animate ? "transform 140ms ease-out" : "none";
		img.style.transform = `translate3d(${next.x}px, ${next.y}px, 0) scale(${next.scale})`;
		following.current = isFitted(next, size, box.current);
		setLevel(Math.round(next.scale * 100));
		setCanPan(pannable(next, size, box.current));
	}, []);

	// Zooms to `scale`, or by what `scale` makes of the current one, around a
	// point in the frame, its centre unless given.
	const zoomTo = useCallback(
		(scale: (current: number) => number, at?: Point, animate = false) => {
			const size = picture.current;
			if (!size) return;
			const { width, height } = box.current;
			const point = at ?? { x: width / 2, y: height / 2 };
			show(
				zoomAt(zoom.current, scale(zoom.current.scale), point, size, box.current),
				animate
			);
		},
		[show]
	);

	const act = useCallback(
		(action: ZoomAction) => {
			const size = picture.current;
			if (!size) return;
			if (action === "fit") show(fitted(size, box.current), true);
			else if (action === "actual") zoomTo(() => 1, undefined, true);
			else zoomTo((s) => (action === "in" ? s * ZOOM_STEP : s / ZOOM_STEP), undefined, true);
		},
		[show, zoomTo]
	);

	const pointIn = (clientX: number, clientY: number): Point => {
		const rect = frame.current!.getBoundingClientRect();
		return { x: clientX - rect.left, y: clientY - rect.top };
	};

	// The frame's size, followed so the picture refits or stays in view.
	useEffect(() => {
		const node = frame.current;
		if (!node) return;
		const measure = () => {
			box.current = { width: node.clientWidth, height: node.clientHeight };
			const size = picture.current;
			if (!size) return;
			show(
				following.current
					? fitted(size, box.current)
					: placed(zoom.current, size, box.current)
			);
		};
		measure();
		if (typeof ResizeObserver === "undefined") return;
		const observer = new ResizeObserver(measure);
		observer.observe(node);
		return () => observer.disconnect();
	}, [show]);

	// Wheels and pinches, which need a listener that may cancel them.
	useEffect(() => {
		const node = frame.current;
		if (!node) return;
		let pinch: number | null = null;
		const onWheel = (event: WheelEvent) => {
			event.preventDefault();
			const size = picture.current;
			if (!size) return;
			const lines = event.deltaMode === WheelEvent.DOM_DELTA_LINE;
			if (event.ctrlKey || event.metaKey) {
				if (pinch !== null) return;
				const factor = wheelFactor(event.deltaY, lines);
				zoomTo((s) => s * factor, pointIn(event.clientX, event.clientY));
			} else {
				const step = lines ? 16 : 1;
				show(
					panBy(
						zoom.current,
						-event.deltaX * step,
						-event.deltaY * step,
						size,
						box.current
					)
				);
			}
		};
		const onGestureStart = (event: Event) => {
			event.preventDefault();
			pinch = zoom.current.scale;
		};
		const onGestureChange = (event: Event) => {
			event.preventDefault();
			const gesture = event as GestureEvent;
			const from = pinch;
			if (from === null) return;
			zoomTo(() => from * gesture.scale, pointIn(gesture.clientX, gesture.clientY));
		};
		const onGestureEnd = (event: Event) => {
			event.preventDefault();
			pinch = null;
		};
		node.addEventListener("wheel", onWheel, { passive: false });
		node.addEventListener("gesturestart", onGestureStart);
		node.addEventListener("gesturechange", onGestureChange);
		node.addEventListener("gestureend", onGestureEnd);
		return () => {
			node.removeEventListener("wheel", onWheel);
			node.removeEventListener("gesturestart", onGestureStart);
			node.removeEventListener("gesturechange", onGestureChange);
			node.removeEventListener("gestureend", onGestureEnd);
		};
	}, [show, zoomTo]);

	// Over the window, the keys are caught before anything else hears them.
	useEffect(() => {
		if (keys !== "window") return;
		const onKey = (event: KeyboardEvent) => {
			if (event.altKey || event.metaKey || event.ctrlKey) return;
			const action = zoomKey(event.key);
			if (!action) return;
			event.preventDefault();
			event.stopPropagation();
			act(action);
		};
		window.addEventListener("keydown", onKey, true);
		return () => window.removeEventListener("keydown", onKey, true);
	}, [keys, act]);

	const onFrameKey = (event: ReactKeyboardEvent) => {
		if (keys !== "focus" || event.altKey || event.metaKey || event.ctrlKey) return;
		const action = zoomKey(event.key);
		if (!action) return;
		event.preventDefault();
		act(action);
	};

	const onPointerDown = (event: ReactPointerEvent) => {
		if (event.button !== 0) return;
		press.current = { x: event.clientX, y: event.clientY, moved: false };
		const size = picture.current;
		if (!size || !pannable(zoom.current, size, box.current)) return;
		event.currentTarget.setPointerCapture(event.pointerId);
		setDragging(true);
	};
	const onPointerMove = (event: ReactPointerEvent) => {
		const held = press.current;
		const size = picture.current;
		if (!held || !size) return;
		const dx = event.clientX - held.x;
		const dy = event.clientY - held.y;
		if (Math.abs(dx) + Math.abs(dy) > CLICK_SLOP) held.moved = true;
		if (!dragging) return;
		press.current = { x: event.clientX, y: event.clientY, moved: held.moved };
		show(panBy(zoom.current, dx, dy, size, box.current));
	};
	const onPointerUp = () => setDragging(false);

	const loaded = level !== null;
	return (
		<div
			ref={frame}
			tabIndex={keys === "focus" ? 0 : undefined}
			aria-label={keys === "focus" ? alt : undefined}
			onKeyDown={onFrameKey}
			onPointerDown={onPointerDown}
			onPointerMove={onPointerMove}
			onPointerUp={onPointerUp}
			onPointerCancel={onPointerUp}
			onClick={(event) => {
				const moved = press.current?.moved ?? false;
				press.current = null;
				if (!moved && event.target === event.currentTarget) onBackground?.();
			}}
			onDoubleClick={(event) => {
				const size = picture.current;
				if (!size || event.target !== image.current) return;
				show(
					toggleActual(
						zoom.current,
						pointIn(event.clientX, event.clientY),
						size,
						box.current
					),
					true
				);
			}}
			className={cn(
				"relative touch-none overflow-hidden outline-none select-none",
				canPan && (dragging ? "cursor-grabbing" : "cursor-grab"),
				className
			)}
		>
			<img
				ref={image}
				src={src}
				alt={alt}
				draggable={false}
				onLoad={(event) => {
					const img = event.currentTarget;
					picture.current = { width: img.naturalWidth, height: img.naturalHeight };
					show(fitted(picture.current, box.current));
				}}
				onError={onError}
				{...(canPan ? {} : drag)}
				style={{ transformOrigin: "0 0" }}
				className={cn(
					"absolute top-0 left-0 max-w-none rounded-md",
					loaded ? "opacity-100" : "opacity-0"
				)}
			/>
			{loaded ? (
				<div
					onPointerDown={(event) => event.stopPropagation()}
					onDoubleClick={(event) => event.stopPropagation()}
					className="absolute right-2 bottom-2 flex items-center gap-0.5 rounded-lg bg-media/60 p-0.5 text-[11.5px] text-media-foreground opacity-50 transition-opacity focus-within:opacity-100 hover:opacity-100"
				>
					<button
						type="button"
						aria-label={translate("Zoom out")}
						title={translate("Zoom out")}
						onClick={() => act("out")}
						className={CONTROL_BUTTON}
					>
						<ZoomOut aria-hidden size={14} strokeWidth={2} />
					</button>
					<button
						type="button"
						aria-label={translate("Actual size")}
						title={translate("Actual size")}
						onClick={() => act("actual")}
						className={cn(CONTROL_BUTTON, "w-[46px] tabular-nums")}
					>
						{formatNumber(level / 100, { style: "percent" })}
					</button>
					<button
						type="button"
						aria-label={translate("Zoom in")}
						title={translate("Zoom in")}
						onClick={() => act("in")}
						className={CONTROL_BUTTON}
					>
						<ZoomIn aria-hidden size={14} strokeWidth={2} />
					</button>
					<button
						type="button"
						aria-label={translate("Zoom to fit")}
						title={translate("Zoom to fit")}
						onClick={() => act("fit")}
						className={CONTROL_BUTTON}
					>
						<Scan aria-hidden size={14} strokeWidth={2} />
					</button>
				</div>
			) : null}
		</div>
	);
};
