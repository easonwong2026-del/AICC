import SwiftUI
import WidgetKit
import AppKit

/// Renders the widget offscreen for layout QA: every state, both families,
/// both appearances.  It never asks Magpie for anything.
@main
struct WidgetVisualSmokeMain {
    static let outputDir = "/private/tmp/widget_qa"

    static func report(named name: String, rows: [(String, String, String?)], kind: String = "subscription") -> QuotaSnapshot {
        let initials = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"]
        let data: [[String: Any]] = rows.enumerated().map { index, row in
            ["provider": row.0, "kind": kind,
             "user": "\(initials[index % initials.count])@example.invalid",
             "plan": row.1,
             "readAt": ISO8601DateFormatter().string(from: .now),
             "balance": row.2 as Any,
             "windows": row.2 == nil ? [["name": "5 hours", "used": 18.0, "remaining": 82.0],
                                        ["name": "7 days", "used": 5.0, "remaining": 95.0,
                                         "resetsAt": ISO8601DateFormatter().string(from: .now.addingTimeInterval(7200))]]
                                      : []]
        }
        let document: [String: Any] = ["object": "list", "data": data]
        let raw = try! JSONSerialization.data(withJSONObject: document)
        return try! MagpieQuotaParser.snapshot(data: raw)
    }

    static var balanceSnapshot: QuotaSnapshot {
        let document: [String: Any] = ["object": "list", "data": [[
            "provider": "deepseek", "kind": "balance", "user": "key@example.invalid",
            "readAt": ISO8601DateFormatter().string(from: .now),
            "balance": "58.70 CNY",
            "balanceSplit": [["label": "paid", "text": "50.00"], ["label": "bonus", "text": "8.70"]]
        ]]]
        let raw = try! JSONSerialization.data(withJSONObject: document)
        return try! MagpieQuotaParser.snapshot(data: raw)
    }

    /// Real numbers for the preview: the bridge's last snapshot if the collector
    /// has run, else whatever the installed widget cached.
    static func cachedSnapshot() -> QuotaSnapshot? {
        let bridge = FileManager.default.currentDirectoryPath + "/data/magpie_last_success.json"
        if let data = FileManager.default.contents(atPath: bridge),
           let snapshot = try? JSONDecoder().decode(QuotaSnapshot.self, from: data) {
            return snapshot
        }
        let path = NSHomeDirectory()
            + "/Library/Containers/com.aieink.dashboard.menubar.widget/Data/Library/Preferences/"
            + "com.aieink.dashboard.menubar.widget.plist"
        guard let data = FileManager.default.contents(atPath: path),
              let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let raw = plist["aicc.widget.quota.3425"] as? Data else { return nil }
        return try? JSONDecoder().decode(QuotaSnapshot.self, from: raw)
    }

    @MainActor
    static func render(_ snapshot: QuotaSnapshot, configuration: AICCWidgetConfiguration,
                       family: WidgetFamily, scheme: ColorScheme, name: String,
                       mode: WidgetRenderingMode = .fullColor) {
        let entry = AICCWidgetEntry(date: .now, snapshot: snapshot, configuration: configuration)
        let width: CGFloat = family == .systemMedium ? 330 : 155
        let size = CGSize(width: width, height: 155)
        // In accented rendering the system removes the widget's background and
        // puts its own glass under the content, so preview it over a wallpaper.
        let accented = mode != .fullColor
        let background: Color = accented
            ? Color(red: 0.16, green: 0.22, blue: 0.29)
            : (scheme == .dark ? Color(red: 0.12, green: 0.12, blue: 0.14) : Color.white)
        let view = AICCWidgetView(entry: entry, familyOverride: family)
            .environment(\.colorScheme, scheme)
            .environment(\.widgetRenderingMode, mode)
            .frame(width: size.width, height: size.height)
            .background(background)
            .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
        let renderer = ImageRenderer(content: view)
        renderer.scale = 2
        guard let image = renderer.nsImage, let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff) else {
            fatalError("Failed to render \(name)")
        }
        precondition(bitmap.pixelsWide == Int(size.width) * 2 && bitmap.pixelsHigh == Int(size.height) * 2,
                     "Unexpected bitmap size for \(name)")
        // A card must actually draw something, and must not fill every pixel:
        // blank and overflowing layouts both fail this.
        var ink = 0
        var sampled = 0
        for y in stride(from: 6, to: bitmap.pixelsHigh - 6, by: 3) {
            for x in stride(from: 6, to: bitmap.pixelsWide - 6, by: 3) {
                guard let pixel = bitmap.colorAt(x: x, y: y)?.usingColorSpace(NSColorSpace.deviceRGB) else { continue }
                sampled += 1
                let luminance = 0.2126 * pixel.redComponent + 0.7152 * pixel.greenComponent + 0.0722 * pixel.blueComponent
                if scheme == .dark || accented {
                    // Accented content is white on the glass: a card that came out
                    // as a solid white block would show almost no dark pixels.
                    if luminance > 0.55 { ink += 1 }
                } else if luminance < 0.55 {
                    ink += 1
                }
            }
        }
        let ratio = Double(ink) / Double(max(1, sampled))
        precondition(ratio > 0.002, "Blank widget: \(name)")
        precondition(ratio < 0.75, "Overfull widget: \(name)")
        if accented {
            // Dimmed on the desktop, an opaque card would be tinted to a solid
            // white slab.  Text is a small share of the tile; a slab is not.
            var solid = 0
            for y in stride(from: 6, to: bitmap.pixelsHigh - 6, by: 3) {
                for x in stride(from: 6, to: bitmap.pixelsWide - 6, by: 3) {
                    guard let pixel = bitmap.colorAt(x: x, y: y)?.usingColorSpace(NSColorSpace.deviceRGB) else { continue }
                    let luminance = 0.2126 * pixel.redComponent + 0.7152 * pixel.greenComponent + 0.0722 * pixel.blueComponent
                    if luminance > 0.9 { solid += 1 }
                }
            }
            let slab = Double(solid) / Double(max(1, sampled))
            precondition(slab < 0.18, "Accented render has solid white slabs (\(slab)): \(name)")
        }
        guard let png = bitmap.representation(using: .png, properties: [:]) else {
            fatalError("Failed to encode \(name)")
        }
        try? png.write(to: URL(fileURLWithPath: "\(outputDir)/\(name).png"))
    }

    @MainActor
    static func main() throws {
        try? FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)

        let mixed = report(named: "mixed", rows: [
            ("codex", "team", nil), ("workbuddy", "Free", nil),
            ("zcode", "GLM Coding Plan", nil), ("brandnew-vendor", "Starter", nil),
            ("antigravity", "Google AI Pro", nil), ("kiro", "Pro", nil),
        ])
        let stale = mixed.evaluated(offline: true)
        let empty = QuotaSnapshot.placeholder()
        let single = report(named: "single", rows: [("codex", "plus", nil)])
        let longLabels = report(named: "long", rows: [
            ("commandcode-plan", "Command Code Max Annual Plan", nil),
            ("workbuddy-ai", "WorkBuddy AI Enterprise Seat", nil),
        ])
        let balances = balanceSnapshot
        var real: [String: QuotaSnapshot] = [:]
        if let cached = cachedSnapshot() { real["real"] = cached }

        let states: [(String, QuotaSnapshot)] = Array(real.map { ($0.key, $0.value) }) + [
            ("mixed", mixed), ("stale", stale), ("empty", empty),
            ("single", single), ("long_labels", longLabels), ("balance", balances),
        ]
        let configurations: [(String, AICCWidgetConfiguration)] = [
            ("auto", AICCWidgetConfiguration()),
            ("one", AICCWidgetConfiguration(cards: .one)),
            ("filtered", AICCWidgetConfiguration(cards: .four, provider: "workbuddy")),
        ]
        let schemes: [(String, ColorScheme)] = [("light", .light), ("dark", .dark)]

        var rendered = 0
        for family in [WidgetFamily.systemSmall, .systemMedium] {
            let familyName = family == .systemMedium ? "medium" : "small"
            for state in states {
                for configuration in configurations {
                    for scheme in schemes {
                        render(state.1, configuration: configuration.1, family: family, scheme: scheme.1,
                               name: "\(familyName)_\(state.0)_\(configuration.0)_\(scheme.0)")
                        rendered += 1
                    }
                }
            }
        }
        // macOS 26 dims desktop widgets into accented rendering whenever another
        // app is in front.  The same layouts have to hold up there too.
        let dimmedStates = states.filter { ["real", "mixed", "balance", "empty", "long_labels"].contains($0.0) }
        for family in [WidgetFamily.systemSmall, .systemMedium] {
            let familyName = family == .systemMedium ? "medium" : "small"
            for state in dimmedStates {
                render(state.1, configuration: AICCWidgetConfiguration(), family: family, scheme: .dark,
                       name: "\(familyName)_\(state.0)_accented", mode: .accented)
                rendered += 1
            }
        }
        print("Visual QA rendered \(rendered) widget PNGs to \(outputDir)")
    }
}
