// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The window's confirmation, as asked before deleting an agent: a
// question, what it means, Cancel and the button that does it. A menu asks
// through `requestConfirm` and waits for the answer. A question with two
// answers, such as Use Auto or Ask first instead, shows the other in
// Cancel's place and asks through `requestAnswer`. A question with
// another way to do it, such as Stop now, shows that beside Cancel. One
// that opens an address shows it in full. One lives with each window.

import { useI18n } from "@maxhq/shared/i18n";

import { AlertDialog } from "@base-ui/react/alert-dialog";
import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { DANGER, SECONDARY } from "@/settings/controls";
import { BACKDROP, POPUP } from "@/workspaces/new-workspace";

/** What a confirmation asks. */
export type Question = {
	/** The question, such as `Delete agent "Release checklist"?`. */
	readonly title: string;
	/** What saying yes means. */
	readonly detail: string;
	/** The button that does it, such as "Delete". */
	readonly action: string;
	/** A second answer in Cancel's place, such as "Ask first instead". */
	readonly other?: string;
	/** Another button that does it differently, beside Cancel, such as "Stop now". */
	readonly alternative?: string;
	/** An address it opens, shown in full under what it means. */
	readonly link?: string;
};

/** Which button you pressed, or null when you closed the question. */
export type Answer = "action" | "other" | "alternative" | null;

type Request = Question & { readonly resolve: (answer: Answer) => void };
type RequestState = { readonly request: Request | null };

const stores = new WeakMap<Services, StoreApi<RequestState>>();

const requestsOf = (services: Services): StoreApi<RequestState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<RequestState>(() => ({ request: null }));
		stores.set(services, store);
	}
	return store;
};

/**
 * Asks the question in the window's confirmation. Resolves with the button
 * you pressed. A new question closes the one before.
 */
export const requestAnswer = (services: Services, question: Question): Promise<Answer> => {
	const store = requestsOf(services);
	store.getState().request?.resolve(null);
	return new Promise((resolve) => store.setState({ request: { ...question, resolve } }));
};

/** Asks the question, and resolves with whether you said yes. */
export const requestConfirm = async (services: Services, question: Question): Promise<boolean> =>
	(await requestAnswer(services, question)) === "action";

/** Whether a question waits for an answer now. */
export const asking = (services: Services): boolean =>
	requestsOf(services).getState().request !== null;

const answer = (services: Services, pressed: Answer) => {
	const store = requestsOf(services);
	const request = store.getState().request;
	store.setState({ request: null });
	request?.resolve(pressed);
};

/** The confirmation, shown while a question waits for an answer. */
export const ConfirmHost = () => {
	const { t } = useI18n();

	const services = useServices();
	const request = useStore(requestsOf(services), (s) => s.request);
	// A question left open when the host goes away answers no.
	useEffect(() => () => answer(services, null), [services]);
	return (
		<AlertDialog.Root
			open={request !== null}
			onOpenChange={(open) => {
				if (!open) answer(services, null);
			}}
		>
			<AlertDialog.Portal>
				<AlertDialog.Backdrop className={BACKDROP} />
				<AlertDialog.Popup className={cn(POPUP, "w-[min(420px,calc(100vw-32px))]")}>
					{request ? (
						<div className="flex flex-col gap-4">
							<div className="flex flex-col gap-1">
								<AlertDialog.Title className="text-[15px] font-[650] text-foreground-strong">
									{request.title}
								</AlertDialog.Title>
								<AlertDialog.Description className="text-[12.5px] text-muted-foreground">
									{request.detail}
								</AlertDialog.Description>
							</div>
							{request.link === undefined ? null : (
								<p className="max-h-[120px] overflow-y-auto rounded-[6px] border border-border bg-subpanel px-2 py-1.5 font-mono text-[11.5px] break-all text-foreground select-text">
									{request.link}
								</p>
							)}
							<div className="flex items-center justify-end gap-2">
								{request.other === undefined ? (
									<AlertDialog.Close className={SECONDARY}>
										{t("Cancel")}
									</AlertDialog.Close>
								) : (
									<button
										type="button"
										className={SECONDARY}
										onClick={() => answer(services, "other")}
									>
										{request.other}
									</button>
								)}
								{request.alternative === undefined ? null : (
									<button
										type="button"
										className={DANGER}
										onClick={() => answer(services, "alternative")}
									>
										{request.alternative}
									</button>
								)}
								<button
									type="button"
									className={DANGER}
									onClick={() => answer(services, "action")}
								>
									{request.action}
								</button>
							</div>
						</div>
					) : null}
				</AlertDialog.Popup>
			</AlertDialog.Portal>
		</AlertDialog.Root>
	);
};
