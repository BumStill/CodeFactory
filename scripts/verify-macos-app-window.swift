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
    case screenCapturePermissionDenied
    case screenshotUnavailable(Int)
    case screenshotBlank(Int, String)
    case cleanupFailed(String)

    var errorDescription: String? {
        switch self {
        case .invalidArguments:
            return "usage: verify-macos-app-window.swift <CodeFactory.app> [timeout-seconds] [evidence-directory]"
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
        case .screenCapturePermissionDenied:
            return "Screen Recording permission is unavailable in this macOS runner session"
        case let .screenshotUnavailable(windowID):
            return "could not capture the verified app window id=\(windowID)"
        case let .screenshotBlank(windowID, reason):
            return "captured app window id=\(windowID) does not prove rendered app content: \(reason)"
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

func writeEvidence(
    window: [String: Any],
    appURL: URL,
    pid: pid_t,
    evidenceDirectory: URL
) throws {
    try FileManager.default.createDirectory(
        at: evidenceDirectory,
        withIntermediateDirectories: true
    )
    guard let windowID = window["window_id"] as? Int else {
        throw SmokeError.screenshotUnavailable(window["window_id"] as? Int ?? -1)
    }
    let screenshotURL = evidenceDirectory.appendingPathComponent("window.png")
    guard CGPreflightScreenCaptureAccess() else {
        throw SmokeError.screenCapturePermissionDenied
    }
    guard let logicalWindowWidth = window["width"] as? Double,
          let logicalWindowHeight = window["height"] as? Double,
          logicalWindowWidth > 0,
          logicalWindowHeight > 0
    else {
        throw SmokeError.screenshotUnavailable(windowID)
    }

    var screenshotWidth = 0
    var screenshotHeight = 0
    var sampledPixels = 0
    var colorBucketCount = 0
    var variedPixels = 0
    var renderedAttempt: Int?
    var lastContentReason = "no screenshot captured"

    // The native window can become stable before the WebView paints. Retry a
    // bounded number of captures and validate only the interior of the actual
    // window, excluding screenshot shadow, title bar, and outer border.
    for attempt in 1...20 {
        try? FileManager.default.removeItem(at: screenshotURL)
        let capture = Process()
        capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        capture.arguments = ["-x", "-l", String(windowID), screenshotURL.path]
        try capture.run()
        capture.waitUntilExit()
        guard capture.terminationStatus == 0,
              let image = NSImage(contentsOf: screenshotURL),
              let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff)
        else {
            throw SmokeError.screenshotUnavailable(windowID)
        }
        screenshotWidth = bitmap.pixelsWide
        screenshotHeight = bitmap.pixelsHigh
        guard screenshotWidth >= 800, screenshotHeight >= 600 else {
            throw SmokeError.screenshotBlank(
                windowID,
                "screenshot is only \(screenshotWidth)x\(screenshotHeight)"
            )
        }

        let scale = max(
            1,
            Int(round(min(
                Double(screenshotWidth) / logicalWindowWidth,
                Double(screenshotHeight) / logicalWindowHeight
            )))
        )
        let windowPixelWidth = Int(logicalWindowWidth) * scale
        let windowPixelHeight = Int(logicalWindowHeight) * scale
        let shadowX = max(0, (screenshotWidth - windowPixelWidth) / 2)
        let shadowY = max(0, (screenshotHeight - windowPixelHeight) / 2)
        let contentMinX = min(screenshotWidth - 1, shadowX + windowPixelWidth / 10)
        let contentMaxX = max(contentMinX + 1, min(screenshotWidth, shadowX + windowPixelWidth * 9 / 10))
        let contentMinY = min(screenshotHeight - 1, shadowY + windowPixelHeight * 15 / 100)
        let contentMaxY = max(contentMinY + 1, min(screenshotHeight, shadowY + windowPixelHeight * 9 / 10))
        let sampleStepX = max(1, (contentMaxX - contentMinX) / 24)
        let sampleStepY = max(1, (contentMaxY - contentMinY) / 24)
        var colorCounts: [String: Int] = [:]
        sampledPixels = 0
        for x in stride(from: contentMinX, to: contentMaxX, by: sampleStepX) {
            for y in stride(from: contentMinY, to: contentMaxY, by: sampleStepY) {
                if let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) {
                    let bucket = "\(Int(color.redComponent * 31))-\(Int(color.greenComponent * 31))-\(Int(color.blueComponent * 31))-\(Int(color.alphaComponent * 31))"
                    colorCounts[bucket, default: 0] += 1
                    sampledPixels += 1
                }
            }
        }
        let dominantPixels = colorCounts.values.max() ?? sampledPixels
        colorBucketCount = colorCounts.count
        variedPixels = sampledPixels - dominantPixels
        if colorBucketCount >= 4,
           sampledPixels > 0,
           variedPixels >= max(4, sampledPixels / 50)
        {
            renderedAttempt = attempt
            break
        }
        lastContentReason = "interior colors=\(colorBucketCount), varied=\(variedPixels)/\(sampledPixels)"
        if attempt < 20 {
            Thread.sleep(forTimeInterval: 1)
        }
    }

    guard let renderedAttempt else {
        throw SmokeError.screenshotBlank(windowID, lastContentReason)
    }

    // Write status=ok only after capture and rendered-content checks pass.
    let metadata: [String: Any] = [
        "status": "ok",
        "proof_tier": ProcessInfo.processInfo.environment["CODEFACTORY_GUI_PROOF_TIER"]
            ?? "remote-real-app-gui",
        "app": appURL.path,
        "pid": Int(pid),
        "stable_seconds": 2,
        "screenshot": [
            "width": screenshotWidth,
            "height": screenshotHeight,
            "content_sample_count": sampledPixels,
            "content_color_buckets": colorBucketCount,
            "content_varied_samples": variedPixels,
            "render_attempt": renderedAttempt,
        ],
        "window": window,
    ]
    let metadataData = try JSONSerialization.data(
        withJSONObject: metadata,
        options: [.prettyPrinted, .sortedKeys]
    )
    try metadataData.write(to: evidenceDirectory.appendingPathComponent("window.json"))
}

func verify() throws {
    guard CommandLine.arguments.count >= 2 else {
        throw SmokeError.invalidArguments
    }

    let appURL = URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL
    let timeout = CommandLine.arguments.count >= 3
        ? Double(CommandLine.arguments[2]) ?? 30
        : 30
    let evidenceDirectory = CommandLine.arguments.count >= 4
        ? URL(fileURLWithPath: CommandLine.arguments[3]).standardizedFileURL
        : nil
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
        let window = try waitForStableMainWindow(app: app, timeout: timeout)
        if let evidenceDirectory {
            try writeEvidence(
                window: window,
                appURL: appURL,
                pid: app.processIdentifier,
                evidenceDirectory: evidenceDirectory
            )
        }
        windowResult = .success(window)
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
