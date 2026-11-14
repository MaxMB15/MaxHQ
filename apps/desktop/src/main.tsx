// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { watchLanguage } from "./app/language";
import { watchTheme } from "./app/theme";
import { ComposerNativeProvider } from "./composer";
import * as native from "./native";
import { watchContextMenu } from "./native/context-guard";
import "./styles/globals.css";

watchTheme();
watchLanguage();
watchContextMenu();

createRoot(document.getElementById("root")!).render(
	<StrictMode>
		<ComposerNativeProvider value={native}>
			<App />
		</ComposerNativeProvider>
	</StrictMode>
);
