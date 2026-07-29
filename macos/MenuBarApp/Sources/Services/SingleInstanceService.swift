import AppKit
import Darwin
import Foundation

/// Holds a process-scoped lock so a second AICC launch cannot create another
/// menu bar extra or start a competing local server.
final class SingleInstanceService {
    static let shared = SingleInstanceService()

    private var lockDescriptor: Int32 = -1

    @discardableResult
    func acquire() -> Bool {
        guard lockDescriptor == -1 else { return true }

        let bundleID = Bundle.main.bundleIdentifier ?? "com.aieink.dashboard.menubar"
        let filename = bundleID.replacingOccurrences(of: ".", with: "-") + ".lock"
        let lockURL = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent(filename)
        let descriptor = Darwin.open(
            lockURL.path,
            O_CREAT | O_RDWR,
            mode_t(S_IRUSR | S_IWUSR)
        )

        guard descriptor >= 0 else {
            NSLog("AICC could not create the single-instance lock at %@", lockURL.path)
            return false
        }

        guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
            Darwin.close(descriptor)
            activateExistingInstance(bundleID: bundleID)
            return false
        }

        lockDescriptor = descriptor
        return true
    }

    deinit {
        guard lockDescriptor >= 0 else { return }
        flock(lockDescriptor, LOCK_UN)
        Darwin.close(lockDescriptor)
    }

    private func activateExistingInstance(bundleID: String) {
        let currentPID = ProcessInfo.processInfo.processIdentifier
        let existing = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
            .first { $0.processIdentifier != currentPID }
        existing?.activate(options: [.activateAllWindows])
    }
}
