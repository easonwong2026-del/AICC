import AppKit
import SwiftUI

/// AICC 主控制台唯一视觉 Token 来源。
/// 页面只引用语义，不直接选择字号、色号或临时尺寸。
enum ConsoleTheme {
    // MARK: 窗口与布局

    static let sidebarWidth: CGFloat = 208
    static let windowSize = NSSize(width: 1120, height: 760)
    static let minWindowSize = NSSize(width: 960, height: 640)
    static let pagePadding: CGFloat = 28
    static let pageTopPadding: CGFloat = 24
    static let pageBottomPadding: CGFloat = 28
    static let headerHeight: CGFloat = 72
    static let sectionSpacing: CGFloat = 24
    static let groupSpacing: CGFloat = 12
    static let rowHeight: CGFloat = 44
    static let accountRowHeight: CGFloat = 52
    static let accountListWidth: CGFloat = 276
    static let detailLabelWidth: CGFloat = 92

    // MARK: 容器与控件

    static let cardRadius: CGFloat = 12
    static let controlRadius: CGFloat = 8
    static let cardPadding: CGFloat = 16
    static let cardSpacing: CGFloat = 16
    static let controlHeight: CGFloat = 30

    enum Sheet {
        static let smallWidth: CGFloat = 440
        static let mediumWidth: CGFloat = 560
        static let padding: CGFloat = 24
    }

    // MARK: 中性色

    static let canvas = adaptive(light: 0xF6F6F7, dark: 0x1B1B1D)
    static let sidebar = adaptive(light: 0xEEEEEF, dark: 0x232325)
    static let card = adaptive(light: 0xFFFFFF, dark: 0x262628)
    static let cardSubtle = adaptive(light: 0xF0F0F2, dark: 0x2C2C2E)
    static let border = adaptive(light: 0xE1E1E4, dark: 0x3A3A3C)
    static let borderStrong = adaptive(light: 0xD2D2D6, dark: 0x48484A)
    static let text = adaptive(light: 0x1D1D1F, dark: 0xF5F5F7)
    static let secondaryText = adaptive(light: 0x606068, dark: 0xA1A1A6)
    static let tertiaryText = adaptive(light: 0x85858B, dark: 0x7C7C80)

    // MARK: 语义色

    static let accent = adaptive(light: 0x007AFF, dark: 0x0A84FF)
    static let accentSoft = adaptive(light: 0xE8F1FD, dark: 0x17324D)
    static let success = adaptive(light: 0x248A3D, dark: 0x30D158)
    static let warning = adaptive(light: 0xC93400, dark: 0xFF9F0A)
    static let danger = adaptive(light: 0xD70015, dark: 0xFF453A)
    static let selectionFill = adaptive(light: 0xDDE9F8, dark: 0x303D4D)
    static let successSoft = adaptive(light: 0xEAF5ED, dark: 0x203529)
    static let warningSoft = adaptive(light: 0xFFF1E8, dark: 0x392A20)
    static let dangerSoft = adaptive(light: 0xFDEBEC, dark: 0x3B2426)

    enum Tone {
        case accent
        case ok
        case warning
        case error
        case neutral
    }

    static func color(_ tone: Tone) -> Color {
        switch tone {
        case .accent: return accent
        case .ok: return success
        case .warning: return warning
        case .error: return danger
        case .neutral: return tertiaryText
        }
    }

    static func softFill(_ tone: Tone) -> Color {
        switch tone {
        case .accent: return accentSoft
        case .ok: return successSoft
        case .warning: return warningSoft
        case .error: return dangerSoft
        case .neutral: return cardSubtle
        }
    }

    static func tone(_ tone: ConsoleAccount.AuthTone) -> Color {
        color(consoleTone(tone))
    }

    static func consoleTone(_ tone: ConsoleAccount.AuthTone) -> Tone {
        switch tone {
        case .ok: return .ok
        case .warning: return .warning
        case .error: return .error
        case .neutral: return .neutral
        }
    }

    private static func adaptive(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return NSColor(rgbHex: isDark ? dark : light)
        })
    }
}

/// 系统字体层级。除特殊图标外，页面不得再创建临时字号。
enum ConsoleType {
    static let pageTitle = Font.system(size: 24, weight: .semibold)
    static let pageSubtitle = Font.system(size: 12)
    static let hero = Font.system(size: 28, weight: .semibold)
    static let detailTitle = Font.system(size: 20, weight: .semibold)
    static let sectionTitle = Font.system(size: 15, weight: .semibold)
    static let sectionDetail = Font.system(size: 12)
    static let rowTitle = Font.system(size: 13, weight: .medium)
    static let body = Font.system(size: 13)
    static let small = Font.system(size: 12)
    static let caption = Font.system(size: 12)
    static let micro = Font.system(size: 11)
    static let metric = Font.system(size: 30, weight: .semibold)
    static let metricSmall = Font.system(size: 20, weight: .semibold)
    static let mono = Font.system(size: 12, design: .monospaced)
    static let monoSmall = Font.system(size: 11, design: .monospaced)
}

extension NSColor {
    convenience init(rgbHex: UInt32) {
        let red = CGFloat((rgbHex >> 16) & 0xFF) / 255
        let green = CGFloat((rgbHex >> 8) & 0xFF) / 255
        let blue = CGFloat(rgbHex & 0xFF) / 255
        self.init(srgbRed: red, green: green, blue: blue, alpha: 1)
    }
}
