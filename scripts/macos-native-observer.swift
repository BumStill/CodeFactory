// Native helper for the non-required, read-only Tauri feasibility slice.
// Private JSON on stdin; callers must not publish raw helper output.
import AppKit
import ApplicationServices
import CryptoKit
import Darwin
import Foundation

enum ProbeFailure: Error { case code(String) }
func require(_ condition: @autoclosure () -> Bool, _ code: String) throws {
    if !condition() { throw ProbeFailure.code(code) }
}
func sha(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
func metadata(_ path: String, directory: Bool) throws -> stat {
    var info = stat()
    try require(lstat(path, &info) == 0, "world_path_unavailable")
    let kind = info.st_mode & S_IFMT
    try require(kind == (directory ? S_IFDIR : S_IFREG), "world_path_type")
    try require(info.st_uid == geteuid() && info.st_mode & 0o022 == 0, "world_path_owner")
    try require(directory || info.st_nlink == 1, "world_path_link")
    try require(URL(fileURLWithPath: path).resolvingSymlinksInPath().path == path, "world_path_alias")
    return info
}
func sealed(_ path: String, maximum: Int) throws -> Data {
    let before = try metadata(path, directory: false)
    let fd = open(path, O_RDONLY | O_NOFOLLOW)
    try require(fd >= 0, "world_file_unavailable")
    defer { close(fd) }
    var opened = stat()
    try require(fstat(fd, &opened) == 0 && opened.st_ino == before.st_ino && opened.st_dev == before.st_dev, "world_file_replaced")
    let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: false)
    let data = try handle.read(upToCount: maximum + 1) ?? Data()
    let after = try metadata(path, directory: false)
    try require(data.count <= maximum && after.st_ino == before.st_ino && after.st_dev == before.st_dev, "world_file_changed")
    return data
}
func text(_ request: [String: Any], _ key: String) throws -> String {
    guard let value = request[key] as? String, !value.isEmpty else { throw ProbeFailure.code("invalid_request") }
    return value
}
func executableDigest(_ path: String) throws -> String {
    let before = try metadata(path, directory: false)
    try require(before.st_size > 0 && before.st_size <= 1024 * 1024 * 1024, "executable_size_invalid")
    let fd = open(path, O_RDONLY | O_NOFOLLOW)
    try require(fd >= 0, "executable_unavailable")
    defer { close(fd) }
    var opened = stat()
    try require(fstat(fd, &opened) == 0 && opened.st_dev == before.st_dev && opened.st_ino == before.st_ino, "executable_replaced")
    let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: false)
    var hasher = SHA256(), count: Int64 = 0
    while let bytes = try handle.read(upToCount: 1024 * 1024), !bytes.isEmpty {
        count += Int64(bytes.count)
        try require(count <= before.st_size, "executable_changed")
        hasher.update(data: bytes)
    }
    let after = try metadata(path, directory: false)
    try require(count == before.st_size && after.st_dev == before.st_dev && after.st_ino == before.st_ino, "executable_changed")
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
}
func verifyWorld(_ request: [String: Any]) throws -> (String, String, String, String) {
    let root = try text(request, "root"), run = try text(request, "run_id")
    let token = try text(request, "owner_token"), identifier = try text(request, "bundle_id")
    try require(UUID(uuidString: run)?.uuidString.lowercased() == run
        && UUID(uuidString: token)?.uuidString.lowercased() == token, "invalid_run")
    try require(identifier == "com.codefactory.scenario." + run.replacingOccurrences(of: "-", with: ""), "invalid_identifier")
    let rootURL = URL(fileURLWithPath: root)
    try require(rootURL.deletingLastPathComponent().path == URL(fileURLWithPath: "/tmp").resolvingSymlinksInPath().path
        && rootURL.lastPathComponent.hasPrefix("codefactory-scenario-"), "invalid_world_root")
    let stat = try metadata(root, directory: true)
    guard let expected = request["root_identity"] as? [String: String] else { throw ProbeFailure.code("invalid_request") }
    try require(expected["device"] == String(stat.st_dev) && expected["inode"] == String(stat.st_ino)
        && stat.st_mode & 0o077 == 0, "world_root_replaced")
    let owner = try sealed(root + "/owner.json", maximum: 65536)
    let manifest = try sealed(root + "/manifest.json", maximum: 65536)
    let ownerSHA = try text(request, "owner_sha256"), manifestSHA = try text(request, "manifest_sha256")
    try require(sha(owner) == ownerSHA && sha(manifest) == manifestSHA, "world_marker_changed")
    guard let parsedOwner = try JSONSerialization.jsonObject(with: owner) as? [String: Any],
          let parsedManifest = try JSONSerialization.jsonObject(with: manifest) as? [String: Any] else { throw ProbeFailure.code("world_marker_schema") }
    try require(parsedOwner["run_id"] as? String == run && parsedOwner["owner_token"] as? String == token
        && parsedManifest["run_id"] as? String == run && parsedManifest["owner_token"] as? String == token
        && parsedManifest["identifier"] as? String == identifier, "world_marker_identity")
    let executable = try text(request, "executable")
    try require(executable == root + "/Probe.app/Contents/MacOS/codefactory", "invalid_executable_path")
    for suffix in ["/Probe.app", "/Probe.app/Contents", "/Probe.app/Contents/MacOS"] {
        _ = try metadata(root + suffix, directory: true)
    }
    return (run, token, identifier, executable)
}
struct ProcessStamp: Equatable {
    let start: String
    let executable: String
}
func processStamp(_ pid: pid_t) throws -> ProcessStamp {
    var info = proc_bsdinfo()
    let size = MemoryLayout<proc_bsdinfo>.stride
    try require(proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(size)) == Int32(size), "process_unavailable")
    // proc_info.h defines PROC_PIDPATHINFO_MAXSIZE as (4 * MAXPATHLEN).
    var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
    try require(proc_pidpath(pid, &buffer, UInt32(buffer.count)) > 0, "process_unavailable")
    let actualPath = String(cString: buffer)
    var after = proc_bsdinfo()
    try require(proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &after, Int32(size)) == Int32(size), "process_unavailable")
    try require(info.pbi_start_tvsec == after.pbi_start_tvsec && info.pbi_start_tvusec == after.pbi_start_tvusec,
        "process_birth_mismatch")
    return ProcessStamp(start: "\(after.pbi_start_tvsec):\(after.pbi_start_tvusec)", executable: actualPath)
}
// All expensive validation must finish before the final OS birth/path read.
// Injected closures make this exact signal boundary testable without an App.
func signalAfterValidation(_ validate: () throws -> (pid_t, ProcessStamp),
                           observe: (pid_t) throws -> ProcessStamp,
                           signal: (pid_t) throws -> Void) throws {
    let (pid, lease) = try validate()
    let fresh = try observe(pid)
    try require(fresh == lease, "signal_identity_changed")
    try signal(pid)
}
func identity(_ request: [String: Any]) throws -> [String: Any] {
    let (run, owner, identifier, executable) = try verifyWorld(request)
    guard let rawPID = request["pid"] as? NSNumber,
          rawPID.doubleValue.rounded() == rawPID.doubleValue,
          rawPID.int64Value > 1, rawPID.int64Value <= Int64(Int32.max) else { throw ProbeFailure.code("invalid_pid") }
    let pid = pid_t(rawPID.int32Value)
    let initial = try processStamp(pid)
    try require(initial.executable == executable, "process_executable_mismatch")
    guard let app = NSRunningApplication(processIdentifier: pid),
          let appBundle = app.bundleURL?.resolvingSymlinksInPath().path,
          let appIdentifier = app.bundleIdentifier else { throw ProbeFailure.code("bundle_unavailable") }
    try require(appBundle == URL(fileURLWithPath: executable).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().path
        && appIdentifier == identifier, "process_bundle_mismatch")
    let digest = try executableDigest(executable), expectedDigest = try text(request, "executable_sha256")
    try require(digest == expectedDigest, "process_digest_mismatch")
    let fresh = try processStamp(pid)
    try require(fresh == initial, "process_identity_changed_during_validation")
    let start = fresh.start
    if let expected = request["start_token"] as? String { try require(expected == start, "process_birth_mismatch") }
    return ["run_id": run, "owner_token": owner, "bundle_id": appIdentifier,
        "pid": Int(pid), "start_token": start, "executable_sha256": digest]
}
func attribute(_ element: AXUIElement, _ key: CFString) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, key, &value) == .success ? value : nil
}
func observeAX(_ request: [String: Any]) throws -> [String: Any] {
    let initial = try identity(request)
    try require(AXIsProcessTrusted(), "accessibility_unavailable")
    let pid = pid_t(initial["pid"] as! Int)
    let application = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(application, 0.2)
    let deadline = Date().addingTimeInterval(1.5)
    var queue: [(AXUIElement, Int)] = [(application, 0)], visited = 0
    var windowSeen = false, settingsSeen = false
    while !queue.isEmpty && visited < 512 && Date() < deadline {
        let (element, depth) = queue.removeFirst()
        visited += 1
        var elementPID: pid_t = 0
        try require(AXUIElementGetPid(element, &elementPID) == .success && elementPID == pid, "ax_pid_mismatch")
        AXUIElementSetMessagingTimeout(element, 0.2)
        let role = attribute(element, kAXRoleAttribute as CFString) as? String
        windowSeen = windowSeen || role == kAXWindowRole as String
        if role == kAXButtonRole as String {
            let title = attribute(element, kAXTitleAttribute as CFString) as? String
            let description = attribute(element, kAXDescriptionAttribute as CFString) as? String
            settingsSeen = settingsSeen || title == "设置" || description == "设置"
        }
        if windowSeen && settingsSeen { break }
        if depth < 14, let children = attribute(element, kAXChildrenAttribute as CFString) as? [AXUIElement] {
            queue.append(contentsOf: children.prefix(512 - visited).map { ($0, depth + 1) })
        }
    }
    var bound = request
    bound["start_token"] = initial["start_token"]
    _ = try identity(bound) // Recheck after the bounded native read operation.
    return ["ax_window_seen": windowSeen, "settings_control": settingsSeen]
}
func preflight() -> [String: Any] {
    let session = CGSessionCopyCurrentDictionary() as? [String: Any]
    let onConsole = session?[kCGSessionOnConsoleKey as String] as? Bool ?? false
    let loggedIn = session?[kCGSessionLoginDoneKey as String] as? Bool ?? false
    let locked = session?["CGSSessionScreenIsLocked"] as? Bool
    return ["accessibility": AXIsProcessTrusted(), "screen_capture": CGPreflightScreenCaptureAccess(),
        "gui_session": onConsole && loggedIn && locked == false]
}
func run() throws -> [String: Any] {
    let bytes = try FileHandle.standardInput.read(upToCount: 65537) ?? Data()
    try require(bytes.count <= 65536, "request_too_large")
    guard let request = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
          let operation = request["operation"] as? String else { throw ProbeFailure.code("invalid_request") }
    switch operation {
    case "preflight": return preflight()
    case "identity": return try identity(request)
    case "observe": return try observeAX(request)
    case "stop", "force-stop":
        _ = try text(request, "start_token")
        try signalAfterValidation({
            let actual = try identity(request)
            return (pid_t(actual["pid"] as! Int), ProcessStamp(start: actual["start_token"] as! String,
                executable: try text(request, "executable")))
        }, observe: processStamp, signal: { pid in
            try require(kill(pid, operation == "stop" ? SIGTERM : SIGKILL) == 0, "owned_signal_failed")
        })
        return ["signal_issued": true]
    default: throw ProbeFailure.code("invalid_operation")
    }
}
#if NATIVE_OBSERVER_CONTRACT_TESTS
do {
    let original = ProcessStamp(start: "1:1", executable: "/synthetic/executable")
    for changed in [ProcessStamp(start: "2:2", executable: original.executable),
                    ProcessStamp(start: original.start, executable: "/synthetic/replaced")] {
        var current = original, signals = 0, rejected = false
        do {
            try signalAfterValidation({
                current = changed // Simulate PID/path replacement during hashing.
                return (123, original)
            }, observe: { _ in current }, signal: { _ in signals += 1 })
        } catch { rejected = true }
        try require(rejected && signals == 0, "fixture_changed_identity_signalled")
    }
    var signals = 0, rejected = false
    do {
        try signalAfterValidation({ (123, original) }, observe: { _ in throw ProbeFailure.code("process_unavailable") },
            signal: { _ in signals += 1 })
    } catch { rejected = true }
    try require(rejected && signals == 0, "fixture_unknown_identity_signalled")
    try signalAfterValidation({ (123, original) }, observe: { _ in original }, signal: { _ in signals += 1 })
    try require(signals == 1, "fixture_matching_identity_not_signalled")
    print("native_signal_contracts_passed")
} catch { print("native_signal_contracts_failed"); exit(2) }
#else
do {
    let result = try run()
    let bytes = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes)
    FileHandle.standardOutput.write(Data("\n".utf8))
} catch {
    let code: String
    if case let ProbeFailure.code(value) = error { code = value } else { code = "native_observation_failed" }
    let bytes = try! JSONSerialization.data(withJSONObject: ["error_code": code], options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes)
    FileHandle.standardOutput.write(Data("\n".utf8))
    exit(3)
}
#endif
