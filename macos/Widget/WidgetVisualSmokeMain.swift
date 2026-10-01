struct AICCWidget {
    static let kind = AICCWidgetMetadata.kind
}

import SwiftUI
import WidgetKit
import AppKit

@main
struct WidgetVisualSmokeMain {
    // Scan the empty left edge of each tile to catch intrinsic-content overflow.
    static func verifyTileSymmetry(_ bitmap: NSBitmapImageRep, name: String) {
        let background = bitmap.colorAt(x: 4, y: 155)!.usingColorSpace(.deviceRGB)!.redComponent
        func tileRows(at x: Int) -> [Int] {
            (20..<290).filter {
                abs(bitmap.colorAt(x: x, y: $0)!.usingColorSpace(.deviceRGB)!.redComponent - background) > 0.008
            }
        }
        let rows = tileRows(at: 28)
        let split = rows.indices.dropFirst().first { rows[$0] - rows[$0 - 1] > 1 }
        guard let split else { fatalError("Missing two tiles: \(name), rows: \(rows)") }
        guard abs(split - (rows.count - split)) <= 1 else {
            fatalError("Unequal tile heights: \(name), top: \(split), bottom: \(rows.count - split)")
        }
        if bitmap.pixelsWide == 660 && rows != tileRows(at: 340) {
            fatalError("Unequal columns: \(name), left: \(rows), right: \(tileRows(at: 340))")
        }
    }

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

        // Synthetic pool data exercises actual slot dimensions without account data.
        let poolJSON = """
        {"codex":{"source":"OpenCodex","selection_mode":"pinned","active_account_id":"plus",
          "accounts":[
            {"id":"team","label":"team","active":false,"weekly":{"remaining":75,"reset":"2026-10-06 17:42"},"five_hour":{"remaining":100}},
            {"id":"plus","label":"plus","active":true,"weekly":{"remaining":42,"reset":"2026-10-04 01:53"},"five_hour":{"remaining":100}}
          ]},"workbuddy":{"points":5000},"deepseek":{"status":"Online","balances":[{"currency":"CNY","total_balance":"60.00"}]},
          "google":{"weekly":{"remaining":90},"five_hour":{"remaining":100},"updated_epoch":1788000000}}
        """
        var pinnedTwo = WidgetDisplaySnapshot(payload: try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(poolJSON.utf8)), fetchedAt: fetchedAt)
        pinnedTwo.codexAccountDisplayMode = .two
        var pinnedOne = pinnedTwo
        pinnedOne.codexAccountDisplayMode = .one
        var autoTwo = pinnedTwo
        autoTwo.codexSelectionMode = "auto"
        autoTwo.codexActiveAccountID = nil
        var autoOne = autoTwo
        autoOne.codexAccountDisplayMode = .one
        var reauth = pinnedTwo
        reauth.codexAccounts = pinnedTwo.codexAccounts?.map {
            WidgetCodexAccount(id: $0.id, label: $0.label, plan: $0.plan, active: $0.active,
                needsReauth: true, stale: true, fiveHour: $0.fiveHour, weekly: $0.weekly)
        }

        let fallbackJSON = """
        {"google":{"five_hour":{"remaining":100},"updated_epoch":1788000000},
         "deepseek":{"status":"Error","error_code":"SERVICE_UNAVAILABLE","balances":[{"currency":"CNY","total_balance":"98765.43"}]},
         "workbuddy":{"points":1234567,"balance_stale":true}}
        """
        let fallbackSnapshot = WidgetDisplaySnapshot(payload: try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(fallbackJSON.utf8)), fetchedAt: fetchedAt)
        var poolFallback = pinnedTwo
        poolFallback.codexAccounts = pinnedTwo.codexAccounts?.map {
            WidgetCodexAccount(id: $0.id, label: $0.label, plan: $0.plan, active: $0.active,
                needsReauth: false, stale: false, fiveHour: $0.fiveHour, weekly: nil)
        }
        var longLabels = pinnedTwo
        longLabels.codexAccounts = pinnedTwo.codexAccounts?.map {
            WidgetCodexAccount(id: $0.id, label: "Very long account label · 长账号名称", plan: $0.plan, active: $0.active,
                needsReauth: false, stale: false, fiveHour: $0.fiveHour, weekly: $0.weekly)
        }

        var snapshots: [(name: String, snapshot: WidgetDisplaySnapshot)] = [

            ("live", liveSnapshot),
            ("stale", staleSnapshot),
            ("nodata", noDataSnapshot),
            ("overflow", overflowSnapshot),
            ("pinned_one", pinnedOne), ("pinned_two", pinnedTwo),
            ("auto_one", autoOne), ("auto_two", autoTwo), ("reauth", reauth),
            ("pool_stale", pinnedTwo.staleCopy), ("fallback", fallbackSnapshot),
            ("pool_5h_fallback", poolFallback), ("long_labels", longLabels)
        ]

        if let input = CommandLine.arguments.dropFirst().first {
            let payload = try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(contentsOf: URL(fileURLWithPath: input)))
            var real = WidgetDisplaySnapshot(payload: payload, fetchedAt: .now)
            real.codexAccountDisplayMode = .two
            snapshots.append(("real_pinned_two", real))
            real.codexAccountDisplayMode = .one
            snapshots.append(("real_pinned_one", real))
        }

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
                            .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))

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
                        verifyTileSymmetry(bitmap, name: filename)
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
                            .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))

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
                        verifyTileSymmetry(bitmap, name: filename)
                        renderedCount += 1
                    }
                }
            }
        }

        print("Visual QA successfully rendered \(renderedCount) widget snapshot PNGs to \(outputDir)")
    }
}
