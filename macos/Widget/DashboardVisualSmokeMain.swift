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
            // 设置页在 Phase 2B 换成四分类 shell：这里继续守「桥接开关 → 关闭语义」，
            // 同时留下两张设置窗口的快速烟测图。完整的 51 张在
            // `scripts/smoke-test-settings-visual.sh`。
            let model = SettingsModel(
                settings: settings,
                server: ServerManager.shared,
                loginAtLaunch: .shared,
                worker: .shared
            )
            for scheme in [ColorScheme.light, .dark] {
                let height: CGFloat = enabled ? 720 : 620
                let view = SettingsView(model: model)
                    .environmentObject(settings)
                    .environmentObject(ServerManager.shared)
                    .environmentObject(LaunchAtLoginService.shared)
                    .environment(\.colorScheme, scheme)
                    .frame(width: 760, height: height)
                let hosting = NSHostingView(rootView: view)
                hosting.frame = NSRect(x: 0, y: 0, width: 760, height: height)
                hosting.layoutSubtreeIfNeeded()
                guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
                    fatalError("Settings rendering failed")
                }
                hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
                guard let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("PNG encoding failed") }
                precondition(bitmap.pixelsWide >= 760 && bitmap.pixelsHigh >= 600)
                let name = "settings-\(enabled ? "bridge" : "widget-only")-\(scheme == .dark ? "dark" : "light").png"
                try png.write(to: URL(fileURLWithPath: outputDir + "/" + name))
            }
        }
        settings.bridgeEnabled = false
        print("Settings visual smoke passed: \(outputDir)")
    }
}
