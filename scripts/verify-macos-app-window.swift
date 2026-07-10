#!/usr/bin/env swift

import AppKit
import CoreGraphics
import Darwin
import Foundation

enum SmokeError: LocalizedError {
    case invalidArguments
    case invalidBundle(String)
    case launchFailed(String)
    case launchedWrongBundle(String, String)
    case processExited(pid_t)
    case windowServerUnavailable
    case windowTimeout(pid_t, [[String: Any]])
    case cleanupFailed(String)

    var errorDescription: String? {
        switch self {
        case .invalidArguments:
            return "usage: verify-macos-app-window.swift <CodeFactory.app> [timeout-seconds]"
        case let .invalidBundle(message), let .launchFailed(message):
            return message
        case let .launchedWrongBundle(actual, expected):
            return "LaunchServices opened '\(actual)', expected exact bundle '\(expected)'"
        case let .processExited(pid):
            return "app process pid=\(pid) exited before the main window became stable"
        case .windowServerUnavailable:
            return "WindowServer is unavailable; runner has no GUI security session"
        case let .windowTimeout(pid, windows):
            return "timed out waiting for a stable 800x600 onscreen layer-0 window for pid=\(pid); observed=\(json(windows))"
        case let .cleanupFailed(message):
            return message
        }
    }
}

func json(_ value: Any) -> String {
    guard JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
          let result = String(data: data, encoding: .utf8)
    else {
        return String(describing: value)
    }
    return result
}

func windowRows(for pid: pid_t, options: CGWindowListOption) throws -> [[String: Any]] {
    guard let rawRows = CGWindowListCopyWindowInfo(options, kCGNullWindowID),
          let rows = rawRows as? [[String: Any]]
    else {
        throw SmokeError.windowServerUnavailable
    }
    return rows.compactMap { row in
        let ownerPID = (row[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value ?? -1
        guard ownerPID == pid else { return nil }

        let layer = (row[kCGWindowLayer as String] as? NSNumber)?.intValue ?? -1
        let alpha = (row[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 0
        let boundsDictionary = row[kCGWindowBounds as String] as? NSDictionary
        let bounds = boundsDictionary.flatMap {
            CGRect(dictionaryRepresentation: $0 as CFDictionary)
        } ?? .zero

        return [
            "window_id": (row[kCGWindowNumber as String] as? NSNumber)?.intValue ?? -1,
            "title": row[kCGWindowName as String] as? String ?? "",
            "layer": layer,
            "alpha": alpha,
            "x": Int(bounds.origin.x),
            "y": Int(bounds.origin.y),
            "width": Int(bounds.width),
            "height": Int(bounds.height),
        ]
    }
}

func runLoop(until condition: () -> Bool, deadline: Date) {
    while !condition() && Date() < deadline {
        RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05))
        Thread.sleep(forTimeInterval: 0.1)
    }
}

func launch(_ appURL: URL, isolatedHome: URL) throws -> NSRunningApplication {
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = true
    configuration.addsToRecentItems = false
    configuration.createsNewApplicationInstance = true
    configuration.environment = [
        "HOME": isolatedHome.path,
        "XDG_CACHE_HOME": isolatedHome.appendingPathComponent(".cache").path,
        "XDG_CONFIG_HOME": isolatedHome.appendingPathComponent(".config").path,
        "XDG_DATA_HOME": isolatedHome.appendingPathComponent(".local/share").path,
    ]

    var launchedApp: NSRunningApplication?
    var launchError: Error?
    var launchFinished = false
    NSWorkspace.shared.openApplication(at: appURL, configuration: configuration) { app, error in
        launchedApp = app
        launchError = error
        launchFinished = true
    }

    runLoop(until: { launchFinished }, deadline: Date().addingTimeInterval(15))
    if let launchError {
        throw SmokeError.launchFailed("failed to launch \(appURL.path): \(launchError)")
    }
    guard let launchedApp else {
        throw SmokeError.launchFailed("LaunchServices did not return an application for \(appURL.path)")
    }

    let actualURL = launchedApp.bundleURL?.resolvingSymlinksInPath().standardizedFileURL
    let expectedURL = appURL.resolvingSymlinksInPath().standardizedFileURL
    guard actualURL == expectedURL else {
        launchedApp.forceTerminate()
        runLoop(until: { launchedApp.isTerminated }, deadline: Date().addingTimeInterval(3))
        throw SmokeError.launchedWrongBundle(actualURL?.path ?? "<unknown>", expectedURL.path)
    }
    return launchedApp
}

func stopAndConfirm(_ app: NSRunningApplication) throws {
    let pid = app.processIdentifier
    if !app.isTerminated {
        app.terminate()
        runLoop(until: { app.isTerminated }, deadline: Date().addingTimeInterval(3))
    }

    if !app.isTerminated {
        app.forceTerminate()
        runLoop(until: { app.isTerminated }, deadline: Date().addingTimeInterval(3))
    }
    guard app.isTerminated else {
        throw SmokeError.cleanupFailed("failed to terminate smoke app pid=\(pid)")
    }

    runLoop(
        until: { (try? windowRows(for: pid, options: [.optionAll]).isEmpty) == true },
        deadline: Date().addingTimeInterval(2)
    )
    let remainingWindows = try windowRows(for: pid, options: [.optionAll])
    guard remainingWindows.isEmpty else {
        throw SmokeError.cleanupFailed(
            "app pid=\(pid) terminated but windows remain: \(json(remainingWindows))"
        )
    }
}

func waitForStableMainWindow(
    app: NSRunningApplication,
    timeout: TimeInterval
) throws -> [String: Any] {
    let pid = app.processIdentifier
    let deadline = Date().addingTimeInterval(timeout)
    var stableWindowID: Int?
    var stableSince: Date?

    while Date() < deadline {
        if app.isTerminated {
            throw SmokeError.processExited(pid)
        }

        let onscreen = try windowRows(
            for: pid,
            options: [.optionOnScreenOnly, .excludeDesktopElements]
        )
        if let candidate = onscreen.first(where: { row in
            (row["layer"] as? Int) == 0
                && (row["alpha"] as? Double ?? 0) > 0
                && (row["width"] as? Int ?? 0) >= 800
                && (row["height"] as? Int ?? 0) >= 600
        }) {
            let candidateID = candidate["window_id"] as? Int
            if candidateID != stableWindowID {
                stableWindowID = candidateID
                stableSince = Date()
            } else if let stableSince, Date().timeIntervalSince(stableSince) >= 2 {
                return candidate
            }
        } else {
            stableWindowID = nil
            stableSince = nil
        }

        Thread.sleep(forTimeInterval: 0.25)
    }

    throw SmokeError.windowTimeout(pid, try windowRows(for: pid, options: [.optionAll]))
}

func verify() throws {
    guard CommandLine.arguments.count >= 2 else {
        throw SmokeError.invalidArguments
    }

    let appURL = URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL
    let timeout = CommandLine.arguments.count >= 3
        ? Double(CommandLine.arguments[2]) ?? 30
        : 30
    guard timeout > 0 else {
        throw SmokeError.invalidArguments
    }

    let infoURL = appURL.appendingPathComponent("Contents/Info.plist")
    guard let info = NSDictionary(contentsOf: infoURL),
          let executableName = info["CFBundleExecutable"] as? String,
          !executableName.isEmpty
    else {
        throw SmokeError.invalidBundle("invalid app bundle at \(appURL.path): missing CFBundleExecutable")
    }

    let executableURL = appURL.appendingPathComponent("Contents/MacOS/\(executableName)")
    guard FileManager.default.isExecutableFile(atPath: executableURL.path) else {
        throw SmokeError.invalidBundle("app executable is missing or not executable: \(executableURL.path)")
    }

    let isolatedHome = appURL.deletingLastPathComponent().appendingPathComponent("smoke-home")
    try FileManager.default.createDirectory(at: isolatedHome, withIntermediateDirectories: true)
    let app = try launch(appURL, isolatedHome: isolatedHome)
    let windowResult: Result<[String: Any], Error>
    do {
        windowResult = .success(try waitForStableMainWindow(app: app, timeout: timeout))
    } catch {
        windowResult = .failure(error)
    }

    let pid = app.processIdentifier
    do {
        try stopAndConfirm(app)
    } catch let cleanupError {
        if case let .failure(verificationError) = windowResult {
            throw SmokeError.cleanupFailed(
                "verification failed: \(verificationError.localizedDescription); cleanup also failed: \(cleanupError.localizedDescription)"
            )
        }
        throw cleanupError
    }

    let evidence = try windowResult.get()
    print(json([
        "status": "ok",
        "app": appURL.path,
        "isolated_home": isolatedHome.path,
        "pid": Int(pid),
        "stable_seconds": 2,
        "window": evidence,
    ]))
}

do {
    try verify()
} catch {
    FileHandle.standardError.write(Data("macOS app window smoke failed: \(error.localizedDescription)\n".utf8))
    exit(1)
}
