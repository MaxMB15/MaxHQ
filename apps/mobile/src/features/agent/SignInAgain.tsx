// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sign in again on an error a signed-out login caused. A login in a config
// folder signs in again in a sheet: the device runs the CLI's own login,
// the phone opens the page, and the sheet takes the code Claude's page shows
// or shows the code to enter for Codex. Cursor needs neither. It asks the
// device every 2 s until the CLI stored the login, and closing it while it
// waits stops the CLI.
// Any other login is sent from Settings on the Mac.

import { useI18n } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import { type ComponentRef, useRef, useState } from "react";
import {
	KeyboardAvoidingView,
	Linking,
	Modal,
	Pressable,
	StyleSheet,
	Text,
	View,
} from "react-native";

import type { Account, AccountId } from "@maxhq/shared/bindings";
import { copyLabel, useCopyWith } from "@maxhq/shared/lib/copy";
import {
	type CliSignInRound,
	opensPageItself,
	SEND_AGAIN,
	signedInText,
	signInAction,
	signInTitle,
	type SignInPage,
	useCliSignIn,
	wantsDeviceCode,
} from "@maxhq/shared/settings/cli-sign-in";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Spinner } from "../../ui/Dot";
import { Input } from "../../ui/Field";
import { opensInBrowser } from "../../ui/markdown";

/** What the error says for a login the phone can't sign in again. */
export const FROM_THE_MAC = "Sign in from Settings on your Mac.";

/** The phone opens a page in the browser. */
const openPage = (url: string) => {
	if (opensInBrowser(url)) Linking.openURL(url).catch(() => {});
};

const PageLink = ({ url }: { url: string }) => {
	const { t } = useI18n();
	return (
		<View style={styles.row}>
			<Button
				label={t("Open the sign-in page")}
				tone="link"
				small
				onPress={() => openPage(url)}
			/>
		</View>
	);
};

const Waiting = ({ text }: { text: string }) => {
	return (
		<View accessibilityRole="progressbar" accessibilityLabel={text} style={styles.waiting}>
			<Spinner size={10} />
			<Text style={styles.muted}>{text}</Text>
		</View>
	);
};

/** Claude's page shows a code to paste back. */
const PasteCode = ({
	page,
	round,
	close,
}: {
	page: SignInPage;
	round: CliSignInRound;
	close: () => void;
}) => {
	const { t } = useI18n();

	// The code stays in the box and this ref until Sign in hands it on.
	const field = useRef<ComponentRef<typeof Input>>(null);
	const code = useRef("");
	const [typed, setTyped] = useState(false);
	const checking = round.state.step === "checking";
	const signIn = () => {
		if (checking || code.current.trim() === "") return;
		const sent = code.current;
		code.current = "";
		field.current?.clear();
		setTyped(false);
		round.finish(sent);
	};
	return (
		<View style={styles.part}>
			<Text style={styles.body}>{t("Your browser opened to sign in.")}</Text>
			<PageLink url={page.url} />
			<Input
				ref={field}
				secureTextEntry
				accessibilityLabel={t("Paste the code from the page")}
				placeholder={t("Paste the code from the page")}
				autoCapitalize="none"
				autoCorrect={false}
				autoComplete="off"
				editable={!checking}
				onChangeText={(text) => {
					code.current = text;
					setTyped(text.trim() !== "");
				}}
				returnKeyType="done"
				onSubmitEditing={signIn}
				style={styles.code}
			/>
			{checking ? <Waiting text={t("Checking the code")} /> : null}
			<View style={styles.buttons}>
				<Button label={t("Cancel")} grow={1} onPress={close} />
				<Button
					label={t("Sign in")}
					tone="primary"
					grow={1}
					disabled={!typed || checking}
					onPress={signIn}
				/>
			</View>
		</View>
	);
};

const CopyCode = ({ code }: { code: string }) => {
	const { t } = useI18n();

	const [state, copy] = useCopyWith(Clipboard.setStringAsync);
	return <Button label={copyLabel(state, t("Copy"))} small onPress={() => copy(code)} />;
};

/** Codex and Cursor sign in on their page and tell the device themselves. */
const FinishThere = ({ page, close }: { page: SignInPage; close: () => void }) => {
	const { t } = useI18n();
	return (
		<View style={styles.part}>
			{page.userCode ? (
				<>
					<Text style={styles.body}>
						{t("Open the sign-in page and enter this code")}
					</Text>
					<View style={styles.userCode}>
						<Text selectable style={styles.userCodeText}>
							{page.userCode}
						</Text>
						<CopyCode code={page.userCode} />
					</View>
				</>
			) : (
				<Text style={styles.body}>
					{t("Your browser opened to sign in. Finish there.")}
				</Text>
			)}
			<PageLink url={page.url} />
			<Waiting text={t("Waiting for you to sign in")} />
			<View style={styles.buttons}>
				<Button label={t("Cancel")} grow={1} onPress={close} />
			</View>
		</View>
	);
};

const Round = ({ account, close }: { account: Account; close: () => void }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const round = useCliSignIn(source, account, {
		deviceCode: wantsDeviceCode(view, account, "phone"),
		opens: opensPageItself(view, account, "phone"),
		open: openPage,
	});
	const { state } = round;
	const device = deviceName(view, account.device);
	return (
		<View style={styles.sheet} accessibilityViewIsModal>
			<View style={styles.head}>
				<Text accessibilityRole="header" style={styles.title}>
					{signInTitle(account.vendor)}
				</Text>
				<Text style={styles.muted}>
					{t("{{account}} on {{device}}", { account: account.label, device })}
				</Text>
			</View>
			{state.step === "starting" || state.step === "cancelled" ? (
				<View style={styles.part}>
					<Waiting text={t("Starting the sign-in on {{device}}", { device })} />
					<View style={styles.buttons}>
						<Button label={t("Cancel")} grow={1} onPress={close} />
					</View>
				</View>
			) : null}
			{state.step === "waiting" || state.step === "checking" ? (
				state.paste ? (
					<PasteCode page={state} round={round} close={close} />
				) : (
					<FinishThere page={state} close={close} />
				)
			) : null}
			{state.step === "signed_in" ? (
				<View style={styles.part}>
					<Text accessibilityRole="alert" style={styles.body}>
						{signedInText(account)} {t(SEND_AGAIN)}
					</Text>
					<View style={styles.buttons}>
						<Button label={t("Done")} tone="primary" grow={1} onPress={close} />
					</View>
				</View>
			) : null}
			{state.step === "failed" ? (
				<View style={styles.part}>
					<Text accessibilityRole="alert" style={styles.failure}>
						{state.message}
					</Text>
					<View style={styles.buttons}>
						<Button label={t("Cancel")} grow={1} onPress={close} />
						<Button
							label={t("Try again")}
							tone="primary"
							grow={1}
							onPress={round.begin}
						/>
					</View>
				</View>
			) : null}
		</View>
	);
};

/** The sheet for one round, open while it's mounted. */
const SignInSheet = ({ id, close }: { id: AccountId; close: () => void }) => {
	const account = useView((s) => s.view.tables.account.get(id));
	return (
		<Modal visible transparent animationType="slide" onRequestClose={close}>
			<KeyboardAvoidingView behavior="padding" style={styles.scrim}>
				<Pressable accessible={false} style={styles.away} onPress={close} />
				{account ? <Round account={account} close={close} /> : null}
			</KeyboardAvoidingView>
		</Modal>
	);
};

/** Sign in again, under an error that names the login it needs. */
export const SignInAgain = ({ account }: { account: AccountId }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const [open, setOpen] = useState(false);
	const action = signInAction(view, account);
	if (!action) return null;
	if (action.kind === "settings") {
		return <Text style={styles.muted}>{action.why ?? t(FROM_THE_MAC)}</Text>;
	}
	return (
		<View style={styles.row}>
			<Button label={t("Sign in again")} small onPress={() => setOpen(true)} />
			{open ? <SignInSheet id={action.account.id} close={() => setOpen(false)} /> : null}
		</View>
	);
};

const styles = StyleSheet.create({
	row: { flexDirection: "row" },
	scrim: { flex: 1, justifyContent: "flex-end", backgroundColor: "rgba(0, 0, 0, 0.55)" },
	away: { flex: 1 },
	sheet: {
		gap: 16,
		padding: 16,
		paddingBottom: 32,
		borderTopLeftRadius: radius.lg,
		borderTopRightRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.card,
	},
	head: { gap: 4 },
	title: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "600" },
	part: { gap: 12, alignItems: "stretch" },
	body: { color: color.foreground, fontSize: size.small, lineHeight: 19 },
	muted: { color: color.mutedForeground, fontSize: size.meta },
	failure: { color: color.errorText, fontSize: size.small, lineHeight: 19 },
	waiting: { flexDirection: "row", alignItems: "center", gap: 8 },
	code: { fontFamily: font.mono, fontSize: size.meta },
	userCode: { flexDirection: "row", alignItems: "center", gap: 12 },
	userCodeText: {
		color: color.foregroundStrong,
		fontFamily: font.mono,
		fontSize: 24,
		fontWeight: "600",
		letterSpacing: 2.5,
	},
	buttons: { flexDirection: "row", gap: 10 },
});
