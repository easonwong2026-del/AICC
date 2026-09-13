struct AICCWidget {
    static let kind = AICCWidgetMetadata.kind
}

import SwiftUI
import WidgetKit
import AppKit

@main
struct WidgetVisualSmokeMain {
    @MainActor
    static func main() throws {
        let outputDir = "/private/tmp/widget_qa"
        try? FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)

        let fetchedAt = Date(timeIntervalSince1970: 1788000000)

        // 1. Live Snapshot
        let livePayloadJSON = """
        {
          "codex": {
            "five_hour": { "remaining": 87, "reset": "14:27" },
            "weekly": { "remaining": 83, "reset": "2026-09-04 08:01" }
          },
          "google": {
            "five_hour": { "remaining": 0, "reset": "2026-09-12 13:00" },
            "weekly": { "remaining": 12, "reset": "2026-09-18 08:00" },
            "updated_epoch": 1788000000
          },
          "workbuddy": { "points": 5760 },
          "deepseek": {
            "status": "Online",
            "balances": [{ "currency": "CNY", "total_balance": "58.70" }]
          },
          "updated_at": "2026-08-29 12:00"
        }
        """
        let liveSnapshot = WidgetDisplaySnapshot(payload: try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(livePayloadJSON.utf8)), fetchedAt: fetchedAt)

        // 2. Stale Snapshot
        let staleSnapshot = liveSnapshot.staleCopy

        // 3. No Data Snapshot
        let noDataPayload = try JSONDecoder().decode(WidgetStatusPayload.self, from: Data("{}".utf8))
        let noDataSnapshot = WidgetDisplaySnapshot(payload: noDataPayload, fetchedAt: fetchedAt)

        // 4. Overflow Snapshot
        let overflowPayloadJSON = """
        {
          "codex": {
            "five_hour": { "remaining": 100, "reset": "2026-09-04 08:01" },
            "weekly": { "remaining": 100, "reset": "2026-09-04 08:01" }
          },
          "google": {
            "five_hour": { "remaining": 100, "reset": "2026-09-12 13:00" },
            "weekly": { "remaining": 100, "reset": "2026-09-18 08:00" },
            "updated_epoch": 1788000000
          },
          "workbuddy": { "points": 1234567 },
          "deepseek": {
            "status": "Online",
            "balances": [{ "currency": "CNY", "total_balance": "98765.43" }]
          }
        }
        """
        let overflowSnapshot = WidgetDisplaySnapshot(payload: try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(overflowPayloadJSON.utf8)), fetchedAt: fetchedAt)

        let snapshots: [(name: String, snapshot: WidgetDisplaySnapshot)] = [
            ("live", liveSnapshot),
            ("stale", staleSnapshot),
            ("nodata", noDataSnapshot),
            ("overflow", overflowSnapshot)
        ]

        let smallConfigs: [(name: String, intent: AICCWidgetConfigurationIntent)] = [
            ("codex_google", AICCWidgetConfigurationIntent(primary: .codex, secondary: .google)),
            ("codex_workbuddy", AICCWidgetConfigurationIntent(primary: .codex, secondary: .workbuddy)),
            ("google_deepseek", AICCWidgetConfigurationIntent(primary: .google, secondary: .deepseek)),
            ("workbuddy_deepseek", AICCWidgetConfigurationIntent(primary: .workbuddy, secondary: .deepseek))
        ]

        let mediumConfigs: [(name: String, intent: AICCWidgetConfigurationIntent)] = [
            ("default", AICCWidgetConfigurationIntent(topLeft: .codex, topRight: .google, bottomLeft: .workbuddy, bottomRight: .deepseek)),
            ("codex_wb_google_ds", AICCWidgetConfigurationIntent(topLeft: .codex, topRight: .workbuddy, bottomLeft: .google, bottomRight: .deepseek)),
            ("wb_ds_codex_google", AICCWidgetConfigurationIntent(topLeft: .workbuddy, topRight: .deepseek, bottomLeft: .codex, bottomRight: .google))
        ]

        let schemes: [(name: String, scheme: ColorScheme)] = [
            ("light", .light),
            ("dark", .dark)
        ]

        let locales: [(name: String, locale: Locale)] = [
            ("zh", Locale(identifier: "zh-Hans")),
            ("en", Locale(identifier: "en"))
        ]

        var renderedCount = 0

        // Render Small
        for s in snapshots {
            for cfg in smallConfigs {
                for sch in schemes {
                    for loc in locales {
                        let entry = AICCWidgetEntry(snapshot: s.snapshot, configuration: cfg.intent)
                        let view = AICCWidgetView(familyOverride: .systemSmall, entry: entry)
                            .environment(\.colorScheme, sch.scheme)
                            .environment(\.locale, loc.locale)
                            .frame(width: 155, height: 155)
                            .background(sch.scheme == .dark ? Color(red: 0.12, green: 0.12, blue: 0.14) : Color.white)

                        let renderer = ImageRenderer(content: view)
                        renderer.scale = 2.0
                        guard let nsImg = renderer.nsImage,
                              let tiff = nsImg.tiffRepresentation,
                              let bitmap = NSBitmapImageRep(data: tiff),
                              let pngData = bitmap.representation(using: .png, properties: [:]) else {
                            fatalError("Failed to render Small image: \(cfg.name)")
                        }
                        let filename = "\(outputDir)/small_\(cfg.name)_\(s.name)_\(sch.name)_\(loc.name).png"
                        try pngData.write(to: URL(fileURLWithPath: filename))
                        renderedCount += 1
                    }
                }
            }
        }

        // Render Medium
        for s in snapshots {
            for cfg in mediumConfigs {
                for sch in schemes {
                    for loc in locales {
                        let entry = AICCWidgetEntry(snapshot: s.snapshot, configuration: cfg.intent)
                        let view = AICCWidgetView(familyOverride: .systemMedium, entry: entry)
                            .environment(\.colorScheme, sch.scheme)
                            .environment(\.locale, loc.locale)
                            .frame(width: 330, height: 155)
                            .background(sch.scheme == .dark ? Color(red: 0.12, green: 0.12, blue: 0.14) : Color.white)

                        let renderer = ImageRenderer(content: view)
                        renderer.scale = 2.0
                        guard let nsImg = renderer.nsImage,
                              let tiff = nsImg.tiffRepresentation,
                              let bitmap = NSBitmapImageRep(data: tiff),
                              let pngData = bitmap.representation(using: .png, properties: [:]) else {
                            fatalError("Failed to render Medium image: \(cfg.name)")
                        }
                        let filename = "\(outputDir)/medium_\(cfg.name)_\(s.name)_\(sch.name)_\(loc.name).png"
                        try pngData.write(to: URL(fileURLWithPath: filename))
                        renderedCount += 1
                    }
                }
            }
        }

        print("Visual QA successfully rendered \(renderedCount) widget snapshot PNGs to \(outputDir)")
    }
}

