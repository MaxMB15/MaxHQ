// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one-time steps for each login, shown as written. Text between
// backticks shows as code: a web address as a link, and a command, a name
// or an address you type somewhere else with a copy icon. The page's text
// doesn't select, so the copy icon is how anything gets out. The steps
// themselves live in the shared package, since the phone shows them too.

import { Fragment } from "react";

import { InlineCopy } from "@/components/copy-field";
import { LOGIN_STEPS, type LoginPage, spanKind } from "@maxhq/shared/settings/login-steps";

import { t as translate, useI18n } from "@maxhq/shared/i18n";

export { LOGIN_STEPS, spanKind };
export type { LoginSteps } from "@maxhq/shared/settings/login-steps";

/** A line of the steps, with its code spans drawn as code. */
export const StepText = ({ text }: { text: string }) => {
	useI18n();
	return (
		<>
			{text.split("`").map((part, i) => {
				if (i % 2 === 0) return <Fragment key={i}>{part}</Fragment>;
				switch (spanKind(part)) {
					case "link":
						return <InlineCopy key={i} text={part} link />;
					case "copy":
						return <InlineCopy key={i} text={part} />;
					default:
						return (
							<code
								key={i}
								className="rounded-[4px] bg-secondary px-1 py-px font-mono text-[11.5px] break-all text-foreground"
							>
								{part}
							</code>
						);
				}
			})}
		</>
	);
};

/** A login's steps as a numbered list, with the notes before and after. */
export const Steps = ({ login }: { login: LoginPage }) => {
	useI18n();
	const { title, before, steps, after } = LOGIN_STEPS[login];
	const notes = [...before, ...after];
	return (
		<section
			aria-label={translate("Steps for {{title}}", { title: title })}
			className="overflow-hidden rounded-[11px] border border-track bg-card"
		>
			<div className="flex items-baseline justify-between gap-3 border-b border-sidebar-border px-4 py-3">
				<h2 className="text-[13.5px] font-[650] text-foreground">{translate("Steps")}</h2>
				<span className="text-[11.5px] text-muted-foreground">
					{translate("Once for each login")}
				</span>
			</div>
			<ol className="flex flex-col gap-2.5 px-4 py-3">
				{steps.map((step, i) => (
					<li
						key={i}
						className="flex gap-2.5 text-[12.5px] leading-[1.55] text-foreground"
					>
						<span
							aria-hidden
							className="grid size-[18px] shrink-0 place-items-center rounded-full bg-secondary text-[10.5px] font-[650] text-secondary-foreground tabular-nums"
						>
							{i + 1}
						</span>
						<span className="min-w-0">
							<StepText text={step} />
						</span>
					</li>
				))}
			</ol>
			{notes.length > 0 ? (
				<details className="group border-t border-sidebar-border px-4 py-2.5">
					<summary className="cursor-pointer text-[11.5px] text-muted-foreground select-none hover:text-foreground">
						{translate("How MaxHQ keeps it")}
					</summary>
					<div className="flex flex-col gap-2 pt-2">
						{notes.map((note, i) => (
							<p
								key={i}
								className="text-[12px] leading-[1.55] text-secondary-foreground"
							>
								<StepText text={note} />
							</p>
						))}
					</div>
				</details>
			) : null}
		</section>
	);
};
