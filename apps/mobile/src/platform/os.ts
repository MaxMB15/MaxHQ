// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The system the phone app runs on, for code that can't import React Native,
// such as the theme and everything the tests load. Metro builds the Android
// app from os.android.ts instead, so this file is what iOS, the type check
// and the tests see.

/** A system the phone app runs on. */
export type PhoneOs = "ios" | "android";

export const OS: PhoneOs = "ios";
