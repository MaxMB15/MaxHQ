// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pairing: what to open on the Mac, a name for this phone, then the camera
// on the Mac's code, or the code pasted when the camera can't be used. The
// core does the rest and answers the devices this phone may now reach. A
// code adds a Mac, or replaces the devices of a Mac the phone pairs with.
// A code pairs only on a tap on Pair, since a sticker can show one and a web
// page can open one as a link: a scanned code asks first, and one opened from
// the Camera app lands in the paste box.

import { useI18n } from "@maxhq/shared/i18n";

import { CameraView, useCameraPermissions } from "expo-camera";
import * as Haptics from "expo-haptics";
import { useMemo, useRef, useState } from "react";
import { Linking, ScrollView, StyleSheet, Text, View } from "react-native";

import type { PhoneBook } from "@maxhq/shared/bindings";
import { describe } from "@maxhq/shared/data/request-source";

import type { PairedMac, PhoneCalls } from "../../data/native-source";
import type { ScreenProps } from "../../navigation";
import { platform } from "../../platform";
import { askToNotify } from "../../push/permission";
import { useServices } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { Input } from "../../ui/Field";
import { Header } from "../../ui/Header";
import { Corners } from "../../ui/Icon";
import { Steps } from "../../ui/Steps";
import { pairNote } from "../macs/macs";
import { refreshMacs, useMacs } from "../macs/useMacs";
import { afterScan, NOT_A_CODE, pairCodeOf } from "./code";

type Step =
	| { readonly step: "ready" }
	| { readonly step: "scanning" }
	| { readonly step: "confirm"; readonly code: string }
	| { readonly step: "pairing" }
	| { readonly step: "paired"; readonly book: PhoneBook }
	| { readonly step: "failed"; readonly message: string };

export const ScanScreen = ({ navigation, route }: ScreenProps<"Scan">) => {
	const { t } = useI18n();

	const { phone } = useServices();
	const [permission, askPermission] = useCameraPermissions();
	const [name, setName] = useState(platform.phoneName);
	const [pasted, setPasted] = useState("");
	const [step, setStep] = useState<Step>({ step: "ready" });
	const [hint, setHint] = useState<string | null>(null);
	// A code opened from the Camera app fills the box, again when another comes.
	const opened = route.params?.code ?? "";
	const [filled, setFilled] = useState("");
	if (opened !== filled) {
		setFilled(opened);
		if (opened) setPasted(opened);
	}
	// A code reads many times a second, so only the first one pairs.
	const busy = useRef(false);
	const { macs } = useMacs();
	const header = (
		<Header
			title={t("Pair")}
			sheet
			back={{ label: t("Close"), onPress: () => navigation.goBack(), close: true }}
		/>
	);

	if (!phone) {
		return (
			<View style={styles.screen}>
				{header}
				<Text style={[styles.body, styles.alone]}>
					{t("The sample data has no phone.")}
				</Text>
			</View>
		);
	}

	const scan = async () => {
		setHint(null);
		const granted = permission?.granted || (await askPermission()).granted;
		if (granted) setStep({ step: "scanning" });
		else setHint(t("MaxHQ needs the camera to scan the code. You can paste the code instead."));
	};

	const found = (text: string) => {
		const next = afterScan(text);
		if ("hint" in next) {
			setHint(next.hint);
			return;
		}
		setHint(null);
		setStep((now) => (now.step === "scanning" ? { step: "confirm", code: next.confirm } : now));
	};

	const pair = async (text: string) => {
		if (busy.current) return;
		const code = pairCodeOf(text);
		if (!code) {
			setHint(t(NOT_A_CODE));
			return;
		}
		busy.current = true;
		setHint(null);
		setStep({ step: "pairing" });
		const result = await phone.pair(code, name.trim() || platform.phoneName);
		setStep(
			result.status === "ok"
				? { step: "paired", book: result.data }
				: { step: "failed", message: describe(result.error) }
		);
		busy.current = false;
		refreshMacs(phone);
		if (result.status !== "ok") return;
		void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
		askToNotify();
	};

	if (step.step === "paired") {
		return (
			<View style={styles.screen}>
				{header}
				<ScrollView contentContainerStyle={styles.content}>
					<Text style={styles.heading}>
						{step.book.mac
							? t("Paired with {{mac}}", { mac: step.book.mac.name })
							: t("Paired")}
					</Text>
					<Text style={styles.body}>{t("This phone can now reach:")}</Text>
					<Card label={t("Devices")}>
						{step.book.devices.map((device, i) => (
							<Text key={device.device} style={[styles.device, i > 0 && styles.line]}>
								{device.name}
							</Text>
						))}
					</Card>
					<Text style={styles.note}>
						{t(
							"MaxHQ can tell you when an agent needs you, stops on an error or a limit, or finishes while you're away. Pick which in Settings."
						)}
					</Text>
					<Button
						label={t("Done")}
						tone="primary"
						onPress={() => navigation.popToTop()}
					/>
				</ScrollView>
			</View>
		);
	}

	return (
		<View style={styles.screen}>
			{header}
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				automaticallyAdjustKeyboardInsets
			>
				{step.step === "scanning" ? (
					<View style={styles.frame}>
						<CameraView
							style={StyleSheet.absoluteFill}
							facing="back"
							barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
							onBarcodeScanned={({ data }) => found(data)}
						/>
						<Corners size={180} length={34} weight={3} tint={color.foregroundStrong} />
						<View style={styles.chip}>
							<Text style={styles.chipText}>{t("Looking for a code")}</Text>
						</View>
					</View>
				) : null}
				{step.step === "confirm" ? (
					<Card>
						<View style={styles.steps}>
							<Text accessibilityRole="header" style={styles.heading}>
								{t("Code found")}
							</Text>
							<Text style={styles.body}>
								{t(
									"Pair only if you just chose Add phone on your own Mac. A code from anywhere else would connect this phone to someone else's devices."
								)}
							</Text>
							<PairNote phone={phone} code={step.code} macs={macs} />
							<Button
								label={t("Pair this phone")}
								tone="primary"
								onPress={() => void pair(step.code)}
							/>
							<Button
								label={t("Cancel")}
								onPress={() => setStep({ step: "ready" })}
							/>
						</View>
					</Card>
				) : null}
				<Card>
					<View style={styles.steps}>
						<Text accessibilityRole="header" style={styles.heading}>
							{t("Pair with your devices")}
						</Text>
						<Steps
							steps={[
								t("On the Mac, open Settings, then Phones."),
								t("Choose Add phone. A code shows."),
								t("Scan it here. It works once, for 10 minutes."),
							]}
						/>
					</View>
				</Card>
				<Text style={styles.note}>
					{t(
						"The phone gets its own key on each device, which lets it reach that device's MaxHQ and nothing else. Remove it on the Mac at any time."
					)}
				</Text>
				<Text style={styles.label}>{t("This phone's name")}</Text>
				<Input
					value={name}
					onChangeText={setName}
					editable={step.step !== "pairing"}
					placeholder={platform.phoneName}
					accessibilityLabel={t("This phone's name")}
				/>
				{step.step === "pairing" ? <Text style={styles.body}>{t("Pairing")}</Text> : null}
				{step.step === "failed" ? <Text style={styles.failed}>{step.message}</Text> : null}
				{hint ? <Text style={styles.note}>{hint}</Text> : null}
				{step.step === "scanning" ? (
					<Button label={t("Stop scanning")} onPress={() => setStep({ step: "ready" })} />
				) : (
					<Button
						label={step.step === "failed" ? t("Scan again") : t("Scan the code")}
						tone="primary"
						icon={<Corners size={14} tint={color.primaryForeground} />}
						disabled={step.step === "pairing"}
						onPress={() => void scan()}
					/>
				)}
				{permission && !permission.granted && !permission.canAskAgain ? (
					<Button
						label={t("Open Settings")}
						onPress={() => void Linking.openSettings()}
					/>
				) : null}
				<Text style={styles.label}>{t("Or paste the code")}</Text>
				<Input
					value={pasted}
					onChangeText={setPasted}
					placeholder="maxhq:..."
					accessibilityLabel={t("Pairing code")}
					autoCapitalize="none"
					autoCorrect={false}
					editable={step.step !== "pairing"}
				/>
				<PairNote phone={phone} code={pairCodeOf(pasted)} macs={macs} />
				<Button
					label={t("Pair")}
					disabled={step.step === "pairing" || pasted.trim() === ""}
					onPress={() => void pair(pasted)}
				/>
			</ScrollView>
		</View>
	);
};

/** Says whether a code adds a Mac or replaces a paired Mac's devices. */
const PairNote = ({
	phone,
	code,
	macs,
}: {
	phone: PhoneCalls;
	code: string | null;
	macs: readonly PairedMac[];
}) => {
	const note = useMemo(() => {
		if (!code) return null;
		const from = phone.pairCodeMac(code);
		return pairNote(from.status === "ok" ? from.data : null, macs);
	}, [phone, code, macs]);
	return note ? <Text style={styles.note}>{note}</Text> : null;
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 12, paddingBottom: 40 },
	alone: { padding: 16 },
	heading: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "700" },
	steps: { paddingHorizontal: 14, paddingVertical: 14, gap: 12 },
	body: { color: color.foreground, fontSize: size.body },
	note: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	label: { color: color.mutedForeground, fontSize: size.meta, fontWeight: "600", marginTop: 4 },
	device: {
		color: color.foregroundStrong,
		fontSize: size.body,
		fontWeight: "600",
		paddingHorizontal: 14,
		paddingVertical: 12,
	},
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	failed: { color: color.errorText, fontSize: size.meta },
	frame: {
		height: 300,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.canvas,
		overflow: "hidden",
		alignItems: "center",
		justifyContent: "center",
	},
	chip: {
		position: "absolute",
		bottom: 14,
		paddingHorizontal: 10,
		paddingVertical: 4,
		borderRadius: 10,
		backgroundColor: tint(color.canvas, 0.75),
	},
	chipText: { color: color.foreground, fontSize: size.meta },
});
