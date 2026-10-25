import SwiftUI
import WidgetKit
import AppKit

/// Offscreen renders of the usage board, one per page and appearance, so a
/// layout mistake shows up before the tile ever reaches the desktop.
@main
struct BoardVisualSmokeMain {
    static let outputDir = "/private/tmp/board_qa"
    static let tile = CGSize(width: 338, height: 354)

    @MainActor
    static func main() throws {
        try? FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)

        let fixture = BoardFixture.snapshot
        var empty = UsageSnapshot.placeholder()
        empty.available = false
        let old = BoardFixture.snapshot.evaluated(at: Date.now.addingTimeInterval(600))

        let states: [(String, UsageSnapshot)] = [("live", fixture), ("empty", empty), ("stale", old)]
        var rendered = 0
        for state in states {
            for page in BoardPage.allCases {
                for (name, scheme) in [("light", ColorScheme.light), ("dark", ColorScheme.dark)] {
                    render(state.1, page: page, scheme: scheme, name: "\(state.0)_\(page)_\(name)")
                    rendered += 1
                }
            }
        }
        for page in BoardPage.allCases {
            render(fixture, page: page, scheme: .dark, mode: .accented, name: "live_\(page)_accented")
            rendered += 1
        }
        // A held page: the footer has to show 手动 without moving the controls.
        for page in BoardPage.allCases {
            render(fixture, page: page, scheme: .dark, manual: true, name: "held_\(page)_dark")
            rendered += 1
        }
        render(fixture, page: .requests, scheme: .light, manual: true, name: "held_requests_light")
        rendered += 1
        print("Board QA rendered \(rendered) PNGs to \(outputDir)")
    }

    @MainActor
    static func render(_ snapshot: UsageSnapshot, page: BoardPage, scheme: ColorScheme,
                       mode: WidgetRenderingMode = .fullColor, manual: Bool = false, name: String) {
        let entry = BoardEntry(date: .now, snapshot: snapshot, page: page, port: 3425, manual: manual)
        let accented = mode != .fullColor
        let backdrop: Color = accented
            ? Color(red: 0.16, green: 0.22, blue: 0.29)
            : (scheme == .dark ? Color(red: 0.12, green: 0.12, blue: 0.14) : Color.white)
        let view = BoardView(entry: entry)
            .environment(\.colorScheme, scheme)
            .environment(\.widgetRenderingMode, mode)
            .frame(width: tile.width, height: tile.height)
            .background(backdrop)
            .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
        let renderer = ImageRenderer(content: view)
        renderer.scale = 2
        guard let image = renderer.nsImage, let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff) else {
            fatalError("Failed to render \(name)")
        }
        precondition(bitmap.pixelsWide == Int(tile.width) * 2 && bitmap.pixelsHigh == Int(tile.height) * 2,
                     "Unexpected bitmap size for \(name)")

        var ink = 0
        var sampled = 0
        var solid = 0
        for y in stride(from: 8, to: bitmap.pixelsHigh - 8, by: 3) {
            for x in stride(from: 8, to: bitmap.pixelsWide - 8, by: 3) {
                guard let pixel = bitmap.colorAt(x: x, y: y)?.usingColorSpace(NSColorSpace.deviceRGB) else { continue }
                sampled += 1
                let luminance = 0.2126 * pixel.redComponent + 0.7152 * pixel.greenComponent + 0.0722 * pixel.blueComponent
                if scheme == .dark || accented {
                    if luminance > 0.55 { ink += 1 }
                } else if luminance < 0.55 {
                    ink += 1
                }
                if accented, luminance > 0.9 { solid += 1 }
            }
        }
        let ratio = Double(ink) / Double(max(1, sampled))
        precondition(ratio > 0.004, "Blank board: \(name)")
        precondition(ratio < 0.8, "Board is flooded with ink: \(name)")
        if accented {
            let slab = Double(solid) / Double(max(1, sampled))
            precondition(slab < 0.2, "Accented board has solid white slabs (\(slab)): \(name)")
        }
        guard let png = bitmap.representation(using: .png, properties: [:]) else {
            fatalError("Failed to encode \(name)")
        }
        try? png.write(to: URL(fileURLWithPath: "\(outputDir)/\(name).png"))
    }
}
