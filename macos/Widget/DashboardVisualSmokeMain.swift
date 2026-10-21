import AppKit
import SwiftUI

@main
struct DashboardVisualSmokeMain {
    @MainActor
    static func main() throws {
        let outputDir = "/private/tmp/aicc-dashboard-qa"
        try FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)
        let settings = AppSettings.shared
        let application = NSApplication.shared
        application.setActivationPolicy(.prohibited)
        let delegate = AICCAppDelegate()
        for enabled in [false, true] {
            settings.bridgeEnabled = enabled
            precondition(delegate.applicationShouldTerminateAfterLastWindowClosed(application) == !enabled)
            for scheme in [ColorScheme.light, .dark] {
                let height: CGFloat = enabled ? 850 : 600
                let view = SettingsView()
                    .environmentObject(settings)
                    .environmentObject(ServerManager.shared)
                    .environmentObject(LaunchAtLoginService.shared)
                    .environment(\.colorScheme, scheme)
                    .frame(width: 580, height: height)
                let hosting = NSHostingView(rootView: view)
                hosting.frame = NSRect(x: 0, y: 0, width: 580, height: height)
                hosting.layoutSubtreeIfNeeded()
                guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
                    fatalError("Settings rendering failed")
                }
                hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
                guard let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("PNG encoding failed") }
                precondition(bitmap.pixelsWide >= 580 && bitmap.pixelsHigh >= 600)
                let name = "settings-\(enabled ? "bridge" : "widget-only")-\(scheme == .dark ? "dark" : "light").png"
                try png.write(to: URL(fileURLWithPath: outputDir + "/" + name))
            }
        }
        settings.bridgeEnabled = false
        print("Settings visual smoke passed: \(outputDir)")
    }
}
