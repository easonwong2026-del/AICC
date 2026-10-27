import Foundation

/// Pure path rules for locating the real installation of a translocated App.
///
/// Kept apart from `WidgetHealthService` so the rule is testable without
/// AppKit, a real translocation or a running WidgetKit daemon.
enum WidgetInstallation {
    static let translocationMarker = "/AppTranslocation/"

    /// True when macOS is running this bundle from its random, read-only copy
    /// under `…/T/AppTranslocation/<UUID>/d/`.
    ///
    /// That only happens while the bundle still carries `com.apple.quarantine`,
    /// and an extension inside such a copy is never registered by `chronod`, so
    /// every Desktop widget stays an empty black tile until the App repairs the
    /// real copy and restarts from it.
    static func isTranslocated(bundlePath: String) -> Bool {
        bundlePath.contains(translocationMarker)
    }

    /// The `/Applications` copy behind a translocated bundle, if it exists.
    ///
    /// `nil` for a normal launch and for a translocated bundle whose installed
    /// copy is missing — both mean "there is nothing better to switch to".
    static func installedPath(forBundlePath bundlePath: String,
                              root: String = "/Applications",
                              exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> String? {
        guard isTranslocated(bundlePath: bundlePath) else { return nil }
        let name = (bundlePath as NSString).lastPathComponent
        guard !name.isEmpty else { return nil }
        let candidate = (root as NSString).appendingPathComponent(name)
        return exists(candidate) ? candidate : nil
    }
}
