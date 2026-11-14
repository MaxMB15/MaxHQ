// swift-tools-version:5.9
// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a push opens, as a package so `swift test` checks it against the
// vectors maxd seals with. The extension's Xcode target builds the same
// source beside NotificationService.swift.

import PackageDescription

let package = Package(
    name: "PushOpen",
    platforms: [.macOS(.v11), .iOS(.v15)],
    products: [.library(name: "PushOpen", targets: ["PushOpen"])],
    targets: [
        .target(name: "PushOpen"),
        .testTarget(name: "PushOpenTests", dependencies: ["PushOpen"]),
    ]
)
