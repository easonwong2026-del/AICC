import AppKit
import SwiftUI

@main
struct DashboardVisualSmokeMain {
    @MainActor
    static func main() throws {
        let outputDir = "/private/tmp/aicc-dashboard-qa"
        try FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)
        let settings = AppSettings.shared
        settings.languageCode = "zh-Hans"
        settings.menuBarShowOpenCodex = false
        let api = APIService(baseURL: "http://127.0.0.1:1")
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        api.status = try JSONDecoder().decode(StatusResponse.self, from: data)
        api.displaySnapshot = WidgetDisplaySnapshot(payload: try JSONDecoder().decode(WidgetStatusPayload.self, from: data), fetchedAt: .now)
        api.state = .ready
        for scheme in [ColorScheme.light, .dark] {
            let view = DashboardView()
                .environmentObject(api)
                .environmentObject(OpenCodexController.shared)
                .environmentObject(settings)
                .environment(\.locale, Locale(identifier: "zh-Hans"))
                .environment(\.colorScheme, scheme)
                .frame(width: 350)
                .background(scheme == .dark ? Color.black : Color.white)
            let hosting = NSHostingView(rootView: view)
            hosting.frame = NSRect(x: 0, y: 0, width: 350, height: hosting.fittingSize.height)
            hosting.layoutSubtreeIfNeeded()
            guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
                fatalError("Dashboard rendering failed")
            }
            hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
            guard let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("PNG encoding failed") }
            precondition(bitmap.pixelsWide >= 350 && bitmap.pixelsHigh > 100 && bitmap.pixelsHigh < 1280)
            try png.write(to: URL(fileURLWithPath: outputDir + "/dashboard-\(scheme == .dark ? "dark" : "light").png"))
        }
        print("Dashboard visual smoke passed: \(outputDir)")
    }
}
