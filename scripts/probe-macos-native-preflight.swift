#!/usr/bin/env swift
// Read-only permission probe. Deliberately has no launch, AX action or cleanup mode.
import ApplicationServices
import CoreGraphics
import Foundation

guard Array(CommandLine.arguments.dropFirst()) == ["--preflight"] else {
    FileHandle.standardError.write(Data("usage: probe-macos-native-preflight.swift --preflight\n".utf8))
    exit(2)
}

// Neither API prompts for permission. Do not replace with a request API.
let accessibility = AXIsProcessTrusted()
let screenCapture = CGPreflightScreenCaptureAccess()
let session = CGSessionCopyCurrentDictionary() as? [String: Any]
let onConsole = session?[kCGSessionOnConsoleKey as String] as? Bool ?? false
let loginComplete = session?[kCGSessionLoginDoneKey as String] as? Bool ?? false
let locked = session?["CGSSessionScreenIsLocked"] as? Bool ?? true
let guiSession = onConsole && loginComplete && !locked
var reasons = ["interactive_driver_not_enabled", "product_startup_isolation_unverified"]
if !accessibility { reasons.append("accessibility_unavailable") }
if !screenCapture { reasons.append("screen_capture_unavailable") }
if !guiSession { reasons.append("unlocked_gui_session_unproven") }
let observation: [String: Any] = [
    "schema_version": 1,
    "scope": "native-desktop-preflight-only",
    "status": "blocked",
    "accessibility": accessibility,
    "screen_capture": screenCapture,
    "gui_session": guiSession,
    "app_launch_count": 0,
    "reason_codes": reasons.sorted(),
]
let data = try JSONSerialization.data(withJSONObject: observation, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data("\n".utf8))
exit(3) // Explicitly not a completed desktop probe or L3 result.
