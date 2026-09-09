#!/usr/bin/env swift
// Read-only permission probe. Deliberately has no launch, AX action or cleanup mode.
import ApplicationServices
import CoreGraphics
import Foundation

guard Array(CommandLine.arguments.dropFirst()) == ["--preflight"] else {
    FileHandle.standardError.write(Data("usage: probe-macos-native-preflight.swift --preflight\n".utf8))
    exit(2)
}

#if NATIVE_PREFLIGHT_CONTRACT_TESTS
// Synthetic compilation only: no WindowServer or permission API is queried.
let input = try JSONSerialization.jsonObject(with: FileHandle.standardInput.readDataToEndOfFile()) as? [String: Any]
guard let fixture = input?["fixture"] as? String,
      ["unknown", "locked", "unlocked", "absent"].contains(fixture) else { exit(2) }
let accessibility = true
let screenCapture = true
var session: [String: Any]? = fixture == "absent" ? nil : [
    kCGSessionOnConsoleKey as String: true, kCGSessionLoginDoneKey as String: true]
if fixture == "locked" || fixture == "unlocked" { session?["CGSSessionScreenIsLocked"] = fixture == "locked" }
#else
// Neither API prompts for permission. Do not replace with a request API.
let accessibility = AXIsProcessTrusted()
let screenCapture = CGPreflightScreenCaptureAccess()
let session = CGSessionCopyCurrentDictionary() as? [String: Any]
#endif
func strictBoolean(_ value: Any?) -> Bool? {
    guard let number = value as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else { return nil }
    return number.boolValue
}
let onConsole = strictBoolean(session?[kCGSessionOnConsoleKey as String]) ?? false
let loginComplete = strictBoolean(session?[kCGSessionLoginDoneKey as String]) ?? false
let locked = strictBoolean(session?["CGSSessionScreenIsLocked"])
let guiSession = onConsole && loginComplete
var reasons = ["interactive_driver_not_enabled", "product_startup_isolation_unverified"]
if !accessibility { reasons.append("accessibility_unavailable") }
if !screenCapture { reasons.append("screen_capture_unavailable") }
if session == nil { reasons.append("no_session_observed") }
else {
    if !guiSession { reasons.append("gui_session_unproven") }
    if locked == true { reasons.append("screen_locked") }
    if locked == nil { reasons.append("unlock_state_unproven") }
}
let observation: [String: Any] = [
    "schema_version": 1,
    "scope": "native-desktop-preflight-only",
    "status": "blocked",
    "accessibility": accessibility,
    "screen_capture": screenCapture,
    "gui_session": guiSession,
    "session_present": session != nil,
    "on_console": onConsole,
    "login_done": loginComplete,
    "lock_state": locked == true ? "locked" : locked == false ? "unlocked" : "unknown",
    "app_launch_count": 0,
    "reason_codes": reasons.sorted(),
]
let data = try JSONSerialization.data(withJSONObject: observation, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data("\n".utf8))
exit(3) // Explicitly not a completed desktop probe or L3 result.
