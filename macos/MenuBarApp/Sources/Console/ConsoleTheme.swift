import AppKit
import SwiftUI

/// 主控制台的**唯一**视觉令牌来源（PRD §8）。
///
/// 规则：
/// - 尺寸只在这里定义，页面里不再写魔法数字；
/// - 颜色全部走 `NSColor` 动态 provider，切外观不需要重建视图树；
/// - 颜色只用来表达状态，不做装饰性大色块。
enum ConsoleTheme {
    // MARK: 尺寸

    static let sidebarWidth: CGFloat = 202
    static let windowSize = NSSize(width: 1120, height: 760)
    static let minWindowSize = NSSize(width: 960, height: 640)

    /// 页面左右边距（PRD 20–24）。
    static let pagePadding: CGFloat = 20
    /// 标题栏高度：保证标题、主操作、齿轮在最小窗口下也完整可见。
    static let headerHeight: CGFloat = 58
    /// 卡片与区块。
    static let cardRadius: CGFloat = 12
    static let cardPadding: CGFloat = 14
    static let cardSpacing: CGFloat = 12
    /// 区块之间的间距（12–16）。
    static let sectionSpacing: CGFloat = 16
    /// 控件统一高度（30–32）。
    static let controlHeight: CGFloat = 28
    /// 列表行高度。
    static let rowHeight: CGFloat = 34
    /// 账号页左栏宽度（PRD 230–250）。
    static let accountListWidth: CGFloat = 244

    // MARK: 颜色

    static let canvas = adaptive(light: 0xF5F6F7, dark: 0x17191C)
    static let sidebar = adaptive(light: 0xEDEFF2, dark: 0x1D2024)
    static let card = adaptive(light: 0xFFFFFF, dark: 0x23262A)
    static let cardSubtle = adaptive(light: 0xF7F8F9, dark: 0x1F2226)
    static let border = adaptive(light: 0xE6E9EC, dark: 0x34383D)
    static let borderStrong = adaptive(light: 0xD8DDE2, dark: 0x3E444A)
    static let text = adaptive(light: 0x20252B, dark: 0xECEEF1)
    static let secondaryText = adaptive(light: 0x7B8490, dark: 0x98A1AD)
    static let tertiaryText = adaptive(light: 0x9AA3AE, dark: 0x7C848E)
    static let accent = adaptive(light: 0x328F80, dark: 0x45B3A0)
    static let accentSoft = adaptive(light: 0xE7F2F0, dark: 0x20332F)
    static let success = adaptive(light: 0x3F9E6A, dark: 0x5CC08A)
    static let warning = adaptive(light: 0xC98430, dark: 0xE0A458)
    static let danger = adaptive(light: 0xC0504D, dark: 0xE0746E)
    static let selectionFill = adaptive(light: 0xDCE5E3, dark: 0x2A3634)

    /// 半透明状态底：用于徽章与内联提示，避免整块上色。
    static let successSoft = adaptive(light: 0xEAF5EE, dark: 0x1E2E26)
    static let warningSoft = adaptive(light: 0xFBF3E6, dark: 0x2E2719)
    static let dangerSoft = adaptive(light: 0xFBECEB, dark: 0x2E1F1F)

    // MARK: 语义

    /// 语义色：页面里只描述"什么状态"，不直接挑色号。
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

    /// 状态点的颜色。
    static func tone(_ tone: ConsoleAccount.AuthTone) -> Color {
        switch tone {
        case .ok: return success
        case .warning: return warning
        case .error: return danger
        case .neutral: return tertiaryText
        }
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

/// 字号与字重（PRD §8：标题 18–20、分组 13–14、正文 12–13、辅助 11–12）。
///
/// 全部集中在这里，禁止在页面里临时写 `size: 9`：10pt 以下的字在最小窗口下读不清。
enum ConsoleType {
    static let pageTitle = Font.system(size: 19, weight: .semibold)
    static let pageSubtitle = Font.system(size: 11)
    static let sectionTitle = Font.system(size: 13, weight: .semibold)
    static let sectionDetail = Font.system(size: 11)
    static let rowTitle = Font.system(size: 12, weight: .medium)
    static let body = Font.system(size: 12)
    static let small = Font.system(size: 11)
    static let caption = Font.system(size: 11)
    static let micro = Font.system(size: 11)
    static let metric = Font.system(size: 22, weight: .semibold)
    static let metricSmall = Font.system(size: 16, weight: .semibold)
    static let mono = Font.system(size: 11, design: .monospaced)
    static let monoSmall = Font.system(size: 10, design: .monospaced)
}

extension NSColor {
    /// 0xRRGGBB → NSColor（sRGB）。
    convenience init(rgbHex: UInt32) {
        let red = CGFloat((rgbHex >> 16) & 0xFF) / 255
        let green = CGFloat((rgbHex >> 8) & 0xFF) / 255
        let blue = CGFloat(rgbHex & 0xFF) / 255
        self.init(srgbRed: red, green: green, blue: blue, alpha: 1)
    }
}
