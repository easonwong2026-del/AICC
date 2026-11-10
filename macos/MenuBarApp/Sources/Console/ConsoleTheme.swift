import AppKit
import SwiftUI

/// 主控制台的配色与尺寸。
///
/// 默认浅色（视觉稿给的是 `#F5F6F7` 画布 + 白色卡片），深色跟随系统。
/// 所有颜色都走 `NSColor` 的动态 provider，切换外观时不需要重建视图树。
enum ConsoleTheme {
    static let sidebarWidth: CGFloat = 202
    static let windowSize = NSSize(width: 1120, height: 760)
    static let minWindowSize = NSSize(width: 960, height: 640)
    static let pagePadding: CGFloat = 26
    static let cardRadius: CGFloat = 14
    static let cardPadding: CGFloat = 18
    static let cardSpacing: CGFloat = 14

    // MARK: 颜色

    static let canvas = adaptive(light: 0xF5F6F7, dark: 0x17191C)
    static let sidebar = adaptive(light: 0xEDEFF2, dark: 0x1D2024)
    static let card = adaptive(light: 0xFFFFFF, dark: 0x23262A)
    static let cardSubtle = adaptive(light: 0xF7F8F9, dark: 0x1F2226)
    static let border = adaptive(light: 0xE6E9EC, dark: 0x34383D)
    static let text = adaptive(light: 0x20252B, dark: 0xECEEF1)
    static let secondaryText = adaptive(light: 0x7B8490, dark: 0x98A1AD)
    static let tertiaryText = adaptive(light: 0x9AA3AE, dark: 0x7C848E)
    static let accent = adaptive(light: 0x328F80, dark: 0x45B3A0)
    static let accentSoft = adaptive(light: 0xE7F2F0, dark: 0x20332F)
    static let success = adaptive(light: 0x3F9E6A, dark: 0x5CC08A)
    static let warning = adaptive(light: 0xC98430, dark: 0xE0A458)
    static let danger = adaptive(light: 0xC0504D, dark: 0xE0746E)
    static let selectionFill = adaptive(light: 0xDCE5E3, dark: 0x2A3634)

    /// 状态点的颜色。
    static func tone(_ tone: ConsoleAccount.AuthTone) -> Color {
        switch tone {
        case .ok: return success
        case .warning: return warning
        case .error: return danger
        case .neutral: return tertiaryText
        }
    }

    private static func adaptive(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return NSColor(rgbHex: isDark ? dark : light)
        })
    }
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

// MARK: - 复用的小组件

/// 卡片容器：白底 + 细边框 + 圆角，不加阴影（PRD 明确要求少阴影）。
struct ConsoleCard<Content: View>: View {
    var padding: CGFloat = ConsoleTheme.cardPadding
    @ViewBuilder var content: Content

    var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: ConsoleTheme.cardRadius))
            .overlay(
                RoundedRectangle(cornerRadius: ConsoleTheme.cardRadius)
                    .strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )
    }
}

/// 状态点 + 文案。
struct ConsoleStatusDot: View {
    let color: Color
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(text).font(.caption).foregroundStyle(ConsoleTheme.secondaryText)
        }
    }
}

/// 页面标题区，右侧可放操作按钮。
struct ConsolePageHeader<Trailing: View>: View {
    let title: String
    let subtitle: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 22, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.text)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 12)
            trailing
        }
    }
}

extension ConsolePageHeader where Trailing == EmptyView {
    init(title: String, subtitle: String) {
        self.init(title: title, subtitle: subtitle) { EmptyView() }
    }
}
