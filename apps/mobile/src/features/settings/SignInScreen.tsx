// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One login's page, as the desktop's Settings has one for each: Claude,
// Codex and Cursor in workspaces, the GitHub App, and phone pushes. Opened
// for one device, such as from Sign in again on its error, the page puts
// what that device holds first.

import { useI18n } from "@maxhq/shared/i18n";

import { ScrollView, StyleSheet, Text, View } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import { LOGIN_HINT, LOGIN_STEPS, type LoginPage } from "@maxhq/shared/settings/login-steps";

import type { ScreenProps } from "../../navigation";
import { color, size } from "../../theme";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Header } from "../../ui/Header";
import { ApnsLogin } from "./login-apns";
import { ClaudeLogin } from "./login-claude";
import { CodexLogin } from "./login-codex";
import { CursorLogin } from "./login-cursor";
import { GithubLogin } from "./login-github";

const Page = ({ login, device }: { login: LoginPage; device?: DeviceId }) => {
	switch (login) {
		case "claude":
			return <ClaudeLogin device={device} />;
		case "codex":
			return <CodexLogin device={device} />;
		case "cursor":
			return <CursorLogin device={device} />;
		case "github":
			return <GithubLogin device={device} />;
		case "apns":
			return <ApnsLogin device={device} />;
	}
};

export const SignInScreen = ({ navigation, route }: ScreenProps<"SignIn">) => {
	const { t } = useI18n();
	const { login, device } = route.params;
	return (
		<View style={styles.screen}>
			<Header
				title={LOGIN_STEPS[login].title}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			<ConnectionBanner />
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				keyboardDismissMode="on-drag"
				automaticallyAdjustKeyboardInsets
			>
				<Text style={styles.hint}>{LOGIN_HINT[login]}</Text>
				<Page login={login} device={device} />
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	hint: { color: color.mutedForeground, fontSize: size.small, paddingHorizontal: 2 },
});
