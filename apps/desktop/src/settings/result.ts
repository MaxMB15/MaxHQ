// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a settings change came back with, in words, and the one guard every
// form with a secret in it goes through before it shows or logs a reply.

export { failureOf, scrub } from "@maxhq/shared/data/failure";

/** A short note under a form. */
export type Note = { readonly text: string; readonly error: boolean };
