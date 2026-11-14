# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

Pod::Spec.new do |s|
  s.name           = 'MaxhqBackground'
  s.version        = '0.0.0'
  s.summary        = 'Background time for answers from a notification'
  s.description    = 'Keeps the app running while an answer from a notification reaches its device'
  s.license        = 'MIT'
  s.author         = 'Max Boksem'
  s.homepage       = 'https://example.com/maxhq'
  s.platforms      = { :ios => '16.4' }
  s.swift_version  = '5.9'
  s.source         = { git: 'https://example.com/maxhq.git' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = '**/*.swift'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
