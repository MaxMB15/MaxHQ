// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { setInterfaceOs } from "@maxhq/shared/i18n";
import App from "./App";
import { watchLanguage } from "./app/language";
import { watchTheme } from "./app/theme";
import { watchWindowScroll } from "./app/window-scroll";
import { ComposerNativeProvider } from "./composer";
import * as native from "./native";
import { watchContextMenu, watchReloadKeys } from "./native/context-guard";
import { appOs } from "./native/platform";
import "./styles/globals.css";

// Before anything draws, so the first frame already has this system's fonts,
// window chrome and wording.
document.documentElement.dataset.os = appOs();
setInterfaceOs(appOs());

watchTheme();
watchLanguage();
watchContextMenu();
watchReloadKeys();
watchWindowScroll();

createRoot(document.getElementById("root")!).render(
	<StrictMode>
		<ComposerNativeProvider value={native}>
			<App />
		</ComposerNativeProvider>
	</StrictMode>
);
