// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the native wrappers fail. They resolve with what there is, an empty
// list or null when there's nothing, and reject with a `NativeFailure` only
// when the call itself went wrong.

import { t } from "@maxhq/shared/i18n";

import { invoke, type InvokeArgs } from "@tauri-apps/api/core";

import type { NativeError } from "@maxhq/shared/bindings";

const message = (reason: NativeError): string => {
	switch (reason.kind) {
		case "unsupported":
			return t("{{what}} isn't available on this platform", { what: reason.what });
		case "not_offered":
			return t("{{path}} wasn't dropped, pasted or picked", { path: reason.path });
		case "failed":
			return reason.message;
	}
};

/** A native command that failed, with the backend's reason. */
export class NativeFailure extends Error {
	/** Why it failed. */
	readonly reason: NativeError;

	constructor(reason: NativeError) {
		super(message(reason));
		this.name = "NativeFailure";
		this.reason = reason;
	}
}

const KINDS: readonly string[] = ["unsupported", "not_offered", "failed"];

const isNativeError = (value: unknown): value is NativeError =>
	typeof value === "object" &&
	value !== null &&
	"kind" in value &&
	typeof value.kind === "string" &&
	KINDS.includes(value.kind);

type Settled<T> = { status: "ok"; data: T } | { status: "error"; error: NativeError };

/** The data from a generated command, or a `NativeFailure` thrown. */
export const unwrap = async <T>(result: Promise<Settled<T>>): Promise<T> => {
	const settled = await result;
	if (settled.status === "error") throw new NativeFailure(settled.error);
	return settled.data;
};

/**
 * Bytes from a command that answers with a raw body. The IPC hands those
 * over as an `ArrayBuffer`, or as a plain array of numbers when it falls
 * back to messages.
 */
export const invokeBytes = async (command: string, args?: InvokeArgs): Promise<Uint8Array> => {
	let body: unknown;
	try {
		body = await invoke<unknown>(command, args);
	} catch (error) {
		throw isNativeError(error) ? new NativeFailure(error) : error;
	}
	if (Array.isArray(body)) return Uint8Array.from(body as number[]);
	if (ArrayBuffer.isView(body))
		return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
	return new Uint8Array(body as ArrayBuffer);
};
