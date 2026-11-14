// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps an error in one part of the window from blanking the rest. The part
// that failed shows a short line in its own spot, with a button that mounts
// it again from the start.

import { useI18n } from "@maxhq/shared/i18n";

import { Component, type ErrorInfo, type ReactNode } from "react";

import { cn } from "@/lib/utils";

const MESSAGE = "This part of MaxHQ hit an error.";

const BUTTON =
	"inline-flex h-7 shrink-0 cursor-pointer items-center rounded-md border border-input bg-secondary px-2.5 text-[12px] text-foreground hover:bg-accent";

type Props = {
	/** What the part is, such as `composer`, for the log. */
	part: string;
	/**
	 * How the line sits in the part's spot: centered in it, or as a strip
	 * like the banners over the composer.
	 */
	look?: "fill" | "strip";
	/** The spot's own classes, for a part with a set width or color. */
	className?: string;
	/** What shows once it fails, in place of the line and the button. */
	fallback?: ReactNode;
	/** Shows the children again when this changes, as when what they read moved on. */
	resetOn?: unknown;
	children: ReactNode;
};

type State = { failed: boolean };

/** Shows its children, or a line and a button to try again once they fail. */
export class ErrorBoundary extends Component<Props, State> {
	state: State = { failed: false };

	static getDerivedStateFromError(): State {
		return { failed: true };
	}

	componentDidCatch(error: unknown, info: ErrorInfo) {
		console.error(`The ${this.props.part} hit an error`, error, info.componentStack);
	}

	componentDidUpdate(previous: Props) {
		if (this.state.failed && previous.resetOn !== this.props.resetOn) this.retry();
	}

	// The failed children were unmounted for the line, so showing them
	// again mounts them from scratch.
	retry = () => this.setState({ failed: false });

	render() {
		if (!this.state.failed) return this.props.children;
		if (this.props.fallback !== undefined) return this.props.fallback;
		return (
			<ErrorFallback
				look={this.props.look}
				className={this.props.className}
				retry={this.retry}
			/>
		);
	}
}

const ErrorFallback = ({
	look,
	className,
	retry,
}: Pick<Props, "look" | "className"> & { retry: () => void }) => {
	const { t } = useI18n();
	const button = (
		<button type="button" className={BUTTON} onClick={retry}>
			{t("Try again")}
		</button>
	);
	if (look === "strip") {
		return (
			<div role="alert" className={cn("shrink-0 px-7 pb-2", className)}>
				<div className="mx-auto flex max-w-[744px] items-center gap-[11px] rounded-[10px] border border-input bg-card py-2.5 pr-3 pl-3.5">
					<p className="min-w-0 flex-1 text-[13px] text-muted-foreground">{t(MESSAGE)}</p>
					{button}
				</div>
			</div>
		);
	}
	return (
		<div role="alert" className={cn("grid h-full place-items-center px-8", className)}>
			<div className="flex flex-col items-center gap-2.5">
				<p className="max-w-[48ch] text-center text-[13px] text-muted-foreground">
					{t(MESSAGE)}
				</p>
				{button}
			</div>
		</div>
	);
};
