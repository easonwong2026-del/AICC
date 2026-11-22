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

    /// Every directory the App may legitimately be installed in, most canonical
    /// first.
    ///
    /// `/Applications` is the DMG's drag target, but `~/Applications` is just as
    /// supported (no admin prompt, and the only workable location on machines
    /// whose `/Applications` is managed).  Looking in one hard-coded directory
    /// meant a translocated launch from `~/Applications` found no installed copy
    /// and quietly gave up, so the widget stayed black until the App was moved.
    static func installRoots(home: String = NSHomeDirectory()) -> [String] {
        ["/Applications", (home as NSString).appendingPathComponent("Applications")]
    }

    /// The installed copy behind a translocated bundle, if there is one.
    ///
    /// `nil` for a normal launch and for a translocated bundle whose installed
    /// copy is missing — both mean "there is nothing better to switch to".
    static func installedPath(forBundlePath bundlePath: String,
                              roots: [String] = installRoots(),
                              exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> String? {
        guard isTranslocated(bundlePath: bundlePath) else { return nil }
        let name = (bundlePath as NSString).lastPathComponent
        guard !name.isEmpty else { return nil }
        for root in roots {
            let candidate = (root as NSString).appendingPathComponent(name)
            if exists(candidate) { return candidate }
        }
        return nil
    }
}
