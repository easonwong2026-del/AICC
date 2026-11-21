import SwiftUI

// MARK: - 设置页布局组件
//
// 只做排版，不引入新的颜色、字号或间距 Token：
// 全部引用 ConsoleTheme / ConsoleType / ConsoleComponents。
// 一套段标题 + 一个单层行容器，行内不再嵌套卡片。

/// 设置页头：简短标题 + 一句辅助。不重复应用名。
struct SettingsPageHeader: View {
    let title: String
    var subtitle: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(ConsoleType.detailTitle)
                .foregroundStyle(ConsoleTheme.text)
            if let subtitle, !subtitle.isEmpty {
                Text(subtitle)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 段：标题 + 单层行容器 + 可选脚注。
struct SettingsGroup<Content: View>: View {
    let title: String
    var detail: String?
    var footnote: String?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ConsoleSectionHeader(title: title, detail: detail) { EmptyView() }
            ConsoleListSurface {
                VStack(alignment: .leading, spacing: 0) { content }
            }
            if let footnote, !footnote.isEmpty {
                Text(footnote)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 2)
            }
        }
    }
}

/// 行：左侧标签（可带一行说明）+ 右侧控件。标签左对齐、控件右对齐。
struct SettingsRow<Control: View>: View {
    let title: String
    var detail: String?
    var detailTone: SettingsTone = .neutral
    var help: String?
    @ViewBuilder var control: Control

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(title)
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                    if let help {
                        Image(systemName: "questionmark.circle")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .help(help)
                    }
                }
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(ConsoleType.small)
                        .foregroundStyle(detailColor)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            control
        }
        .padding(.vertical, 8)
        .frame(minHeight: 34)
    }

    private var detailColor: Color {
        switch detailTone {
        case .neutral: return ConsoleTheme.secondaryText
        case .ok: return ConsoleTheme.success
        case .warning: return ConsoleTheme.warning
        case .error: return ConsoleTheme.danger
        case .accent: return ConsoleTheme.accent
        }
    }
}

/// 只读值行：右对齐的值，超长折两行。
struct SettingsValueRow: View {
    let title: String
    let value: String
    var tone: SettingsTone = .neutral
    var mono: Bool = false
    var detail: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            Text(value)
                .font(mono ? ConsoleType.mono : ConsoleType.small)
                .foregroundStyle(toneColor)
                .multilineTextAlignment(.trailing)
                .lineLimit(2)
                .truncationMode(.middle)
                .layoutPriority(1)
        }
        .padding(.vertical, 8)
        .frame(minHeight: 34)
    }

    private var toneColor: Color {
        switch tone {
        case .neutral: return ConsoleTheme.secondaryText
        case .ok: return ConsoleTheme.success
        case .warning: return ConsoleTheme.warning
        case .error: return ConsoleTheme.danger
        case .accent: return ConsoleTheme.accent
        }
    }
}

/// 行间分隔线：只画在容器内部，不额外包一层。
struct SettingsRowDivider: View {
    var body: some View {
        Divider().opacity(0.6)
    }
}

/// 行内异常说明：贴在对应设置项下方，不遮界面。
struct SettingsInlineNotice: View {
    let kind: ConsoleInlineNotice.Kind
    let message: String
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        ConsoleInlineNotice(kind: kind, message: message, actionTitle: actionTitle, action: action)
    }
}

/// 危险动作区：与普通按钮分开，风险说明不可省。
struct SettingsDangerRow: View {
    let title: String
    let detail: String
    let buttonTitle: String
    var disabled: Bool = false
    let action: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(title)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Spacer(minLength: 8)
                Button(buttonTitle, action: action)
                    .buttonStyle(SettingsDestructiveButtonStyle())
                    .disabled(disabled)
            }
            Text(detail)
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, 8)
    }
}

/// 危险按钮：红字 + 淡红描边，靠样式区分，不靠填充抢注意力。
struct SettingsDestructiveButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        StyleBody(configuration: configuration)
    }

    private struct StyleBody: View {
        @Environment(\.isEnabled) private var isEnabled
        let configuration: Configuration

        var body: some View {
            configuration.label
                .font(ConsoleType.body)
                .lineLimit(1)
                .foregroundStyle(isEnabled ? ConsoleTheme.danger : ConsoleTheme.tertiaryText)
                .padding(.horizontal, 11)
                .frame(height: ConsoleTheme.controlHeight)
                .background(
                    (configuration.isPressed && isEnabled) ? ConsoleTheme.dangerSoft : ConsoleTheme.card,
                    in: RoundedRectangle(cornerRadius: ConsoleTheme.controlRadius)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: ConsoleTheme.controlRadius)
                        .strokeBorder(
                            isEnabled ? ConsoleTheme.danger.opacity(0.35) : ConsoleTheme.border,
                            lineWidth: 0.5
                        )
                )
                .opacity(isEnabled ? 1 : 0.7)
        }
    }
}

/// 窄窗口的顶部分类选择器。入口一个都不能少。
struct SettingsCompactCategoryPicker: View {
    @Binding var selection: SettingsCategory

    var body: some View {
        Picker("分类", selection: $selection) {
            ForEach(SettingsCategory.allCases) { category in
                Label(category.title, systemImage: category.symbol).tag(category)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .padding(.horizontal, SettingsWindowMetrics.contentPadding)
        .padding(.vertical, 10)
    }
}

/// 宽窗口的左侧分类导航。
struct SettingsCategoryNavigation: View {
    @Binding var selection: SettingsCategory

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(SettingsCategory.allCases) { category in
                Button {
                    selection = category
                } label: {
                    HStack(alignment: .center, spacing: 8) {
                        Image(systemName: category.symbol)
                            .font(.system(size: 12))
                            .frame(width: 16)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(category.title)
                                .font(ConsoleType.rowTitle)
                                .lineLimit(1)
                            Text(category.summary)
                                .font(ConsoleType.micro)
                                .foregroundStyle(selection == category
                                                 ? ConsoleTheme.secondaryText
                                                 : ConsoleTheme.tertiaryText)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(selection == category ? ConsoleTheme.text : ConsoleTheme.secondaryText)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        selection == category ? ConsoleTheme.selectionFill : Color.clear,
                        in: RoundedRectangle(cornerRadius: ConsoleTheme.controlRadius)
                    )
                    .contentShape(RoundedRectangle(cornerRadius: ConsoleTheme.controlRadius))
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == category ? [.isSelected] : [])
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 14)
        .frame(width: SettingsWindowMetrics.navigationWidth, alignment: .leading)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(ConsoleTheme.sidebar)
    }
}
