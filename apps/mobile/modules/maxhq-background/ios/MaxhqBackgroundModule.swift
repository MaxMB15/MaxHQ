// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps the app running in the background while an answer from a
// notification's buttons reaches its device. iOS wakes the app for the
// tap, and without this it may suspend the app again before the link is
// up. iOS ends the time on its own after about half a minute.

import ExpoModulesCore
import UIKit

public class MaxhqBackgroundModule: Module {
  private var task: UIBackgroundTaskIdentifier = .invalid
  private var holds = 0

  public func definition() -> ModuleDefinition {
    Name("MaxhqBackground")

    AsyncFunction("hold") {
      self.hold()
    }.runOnQueue(.main)

    AsyncFunction("release") {
      self.release()
    }.runOnQueue(.main)
  }

  private func hold() {
    holds += 1
    guard task == .invalid else { return }
    task = UIApplication.shared.beginBackgroundTask(withName: "Answer") { [weak self] in
      self?.end()
    }
  }

  private func release() {
    holds = max(0, holds - 1)
    if holds == 0 { end() }
  }

  private func end() {
    holds = 0
    guard task != .invalid else { return }
    UIApplication.shared.endBackgroundTask(task)
    task = .invalid
  }
}
