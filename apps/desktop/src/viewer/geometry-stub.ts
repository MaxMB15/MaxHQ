// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Stands in for @file-viewer/geometry-engine in the viewer's build. That
// package reads STEP, IGES and BREP models through occt-import-js, whose
// license (LGPL-2.1) the app can't ship, so the 3D renderer gets this
// instead and treats those formats as ones it doesn't know. The app's list
// of viewer files leaves them out too.

import { t } from "@maxhq/shared/i18n";

type Inspection = {
	readonly format?: string;
	readonly signature?: string;
	readonly warnings: readonly string[];
};

const UNSUPPORTED = "MaxHQ doesn't open CAD formats such as STEP. Open the file in an editor.";

export const isGeometryKernelFormat = (): boolean => false;

export const inspectGeometryKernelFile = (_input: unknown, type?: string): Inspection => ({
	format: type,
	warnings: [],
});

export const formatGeometryKernelNotice = (): string => UNSUPPORTED;

export const importOcctGeometryFile = async (): Promise<never> => {
	throw new Error(t(UNSUPPORTED));
};
