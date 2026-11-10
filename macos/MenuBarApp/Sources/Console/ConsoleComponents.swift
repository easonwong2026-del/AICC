import SwiftUI

/// 跨页面复用的小组件。集中放这里，避免每个页面各写一套样式。
struct ConsoleSectionTitle: View {
    let title: String
    var detail: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(ConsoleTheme.text)
            if let detail {
                Text(detail)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 0)
        }
    }
}

struct ConsoleStatTile: View {
    let caption: String
    let value: String
    var hint: String?
    var tone: Color = ConsoleTheme.text

    var body: some View {
        ConsoleCard(padding: 14) {
            VStack(alignment: .leading, spacing: 5) {
                Text(caption)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Text(value)
                    .font(.system(size: 26, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(tone)
                if let hint {
                    Text(hint)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                }
            }
        }
    }
}

struct ConsoleBadge: View {
    let text: String
    var tint: Color

    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(tint)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(tint.opacity(0.12), in: Capsule())
    }
}

/// 空状态：永远说明"为什么没有数据"和"下一步能做什么"，不放占位数字。
struct ConsoleEmptyState: View {
    let symbol: String
    let title: String
    let message: String
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: symbol)
                .font(.system(size: 26, weight: .light))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Text(title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(ConsoleTheme.text)
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .buttonStyle(.borderless)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(ConsoleTheme.accent)
                    .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 34)
        .padding(.horizontal, 20)
    }
}

/// 额度条：`share` 是剩余百分比。
struct ConsoleShareBar: View {
    let share: Double
    var tone: Color = ConsoleTheme.accent

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(ConsoleTheme.border)
                Capsule()
                    .fill(tone)
                    .frame(width: max(2, geometry.size.width * min(1, max(0, share / 100))))
            }
        }
        .frame(height: 3)
    }
}

/// 主按钮 / 次按钮统一样式，保证深色浅色下都清楚。
/// `isEnabled` 必须参与配色：未接入的按钮要一眼看出点不了（PRD §三.4）。
struct ConsolePrimaryButtonStyle: ButtonStyle {
    var enabled: Bool = true

    func makeBody(configuration: Configuration) -> some View {
        StyleBody(configuration: configuration, enabled: enabled)
    }

    private struct StyleBody: View {
        @Environment(\.isEnabled) private var isEnabled
        let configuration: Configuration
        let enabled: Bool

        var body: some View {
            let active = enabled && isEnabled
            configuration.label
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(active ? Color.white : ConsoleTheme.tertiaryText)
                .padding(.horizontal, 11)
                .frame(height: 25)
                .background(
                    active
                        ? ConsoleTheme.accent.opacity(configuration.isPressed ? 0.82 : 1)
                        : ConsoleTheme.cardSubtle,
                    in: RoundedRectangle(cornerRadius: 7)
                )
        }
    }
}

struct ConsoleSecondaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        StyleBody(configuration: configuration)
    }

    private struct StyleBody: View {
        @Environment(\.isEnabled) private var isEnabled
        let configuration: Configuration

        var body: some View {
            configuration.label
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(isEnabled ? ConsoleTheme.text : ConsoleTheme.tertiaryText)
                .padding(.horizontal, 11)
                .frame(height: 25)
                .background(
                    (configuration.isPressed && isEnabled) ? ConsoleTheme.cardSubtle : ConsoleTheme.card,
                    in: RoundedRectangle(cornerRadius: 7)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 7).strokeBorder(ConsoleTheme.border, lineWidth: 1)
                )
                .opacity(isEnabled ? 1 : 0.65)
        }
    }
}
