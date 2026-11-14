// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/** Joins class names, and lets a later Tailwind class override an earlier one. */
export const cn = (...inputs: ClassValue[]) => twMerge(clsx(inputs));
