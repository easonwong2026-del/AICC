import SwiftUI

/// 跨页面复用的组件（PRD §8）。页面里不允许再自建一套样式。

// MARK: - 卡片

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

// MARK: - 分组标题

struct ConsoleSectionTitle: View {
    let title: String
    var detail: String?

    var body: some View {
        ConsoleSectionHeader(title: title, detail: detail) { EmptyView() }
    }
}

/// 分组标题 + 右侧操作。四个页面统一用它起头。
struct ConsoleSectionHeader<Trailing: View>: View {
    let title: String
    var detail: String?
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title)
                .font(ConsoleType.sectionTitle)
                .foregroundStyle(ConsoleTheme.text)
                .lineLimit(1)
            if let detail, !detail.isEmpty {
                Text(detail)
                    .font(ConsoleType.sectionDetail)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            trailing
        }
    }
}

// MARK: - 统计卡

struct ConsoleStatTile: View {
    let caption: String
    let value: String
    var hint: String?
    var tone: Color = ConsoleTheme.text
    /// 值不是数字（如 `—`）时不再放大，避免看起来像有数据。
    var emphasized: Bool = true

    var body: some View {
        ConsoleCard(padding: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(caption)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
                Text(value)
                    .font(emphasized ? ConsoleType.metric : ConsoleType.metricSmall)
                    .monospacedDigit()
                    .foregroundStyle(tone)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                if let hint {
                    Text(hint)
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}

// MARK: - 徽章 / 状态点

struct ConsoleBadge: View {
    let text: String
    var tint: Color
    var soft: Bool = true

    var body: some View {
        Text(text)
            .font(ConsoleType.caption)
            .foregroundStyle(tint)
            .lineLimit(1)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(tint.opacity(soft ? 0.12 : 0), in: Capsule())
    }
}

struct ConsoleStatusDot: View {
    let color: Color
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(text)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .lineLimit(1)
        }
    }
}

// MARK: - 空状态

/// 空状态永远说明"为什么没有"和"下一步做什么"，不放占位数字。
struct ConsoleEmptyState: View {
    let symbol: String
    let title: String
    let message: String
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 22, weight: .light))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Text(title)
                .font(ConsoleType.sectionTitle)
                .foregroundStyle(ConsoleTheme.text)
            Text(message)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 26)
        .padding(.horizontal, 16)
    }
}

/// 内联提示：错误、警告、信息都走这里，尺寸一致，不遮界面。
struct ConsoleInlineNotice: View {
    enum Kind {
        case info
        case warning
        case error

        var tone: ConsoleTheme.Tone {
            switch self {
            case .info: return .neutral
            case .warning: return .warning
            case .error: return .error
            }
        }

        var symbol: String {
            switch self {
            case .info: return "info.circle"
            case .warning: return "exclamationmark.triangle"
            case .error: return "exclamationmark.octagon"
            }
        }
    }

    let kind: Kind
    let message: String
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: kind.symbol)
                .font(ConsoleType.caption)
            Text(message)
                .font(ConsoleType.small)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .buttonStyle(.borderless)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.accent)
            }
        }
        .foregroundStyle(kind == .error ? ConsoleTheme.danger : ConsoleTheme.secondaryText)
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            ConsoleTheme.softFill(kind.tone).opacity(kind == .info ? 0.7 : 1),
            in: RoundedRectangle(cornerRadius: 8)
        )
    }
}

// MARK: - 详情分组

/// 账号详情的 A/B/C/D 区块：标题 + 卡片内容，间距统一。
struct ConsoleDetailGroup<Content: View>: View {
    let title: String
    var detail: String?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ConsoleSectionHeader(title: title, detail: detail) { EmptyView() }
            ConsoleCard(padding: 12) {
                content
            }
        }
    }
}

/// 详情里的一行 `标签 值`；值过长自动折行，不顶破卡片。
struct ConsoleInfoRow: View {
    let label: String
    let value: String
    var mono: Bool = false
    var valueTint: Color = ConsoleTheme.text

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(label)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 78, alignment: .leading)
            Text(value)
                .font(mono ? ConsoleType.mono : ConsoleType.body)
                .foregroundStyle(valueTint)
                .lineLimit(2)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
    }
}

// MARK: - 搜索框 / 筛选菜单

/// 自适应搜索框：宽度跟随工具栏，不再写死 200pt（PRD §4.1）。
struct ConsoleSearchField: View {
    let placeholder: String
    @Binding var text: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(ConsoleType.body)
                .foregroundStyle(ConsoleTheme.text)
            if !text.isEmpty {
                Button {
                    text = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 8)
        .frame(height: ConsoleTheme.controlHeight)
        .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 7))
        .overlay(
            RoundedRectangle(cornerRadius: 7).strokeBorder(ConsoleTheme.border, lineWidth: 1)
        )
    }
}

/// 紧凑筛选菜单：替代固定 420pt 的 segmented control（PRD §4.1）。
struct ConsoleFilterMenu<Option: Identifiable & Equatable>: View {
    let title: String
    let options: [Option]
    let label: (Option) -> String
    @Binding var selection: Option

    var body: some View {
        Menu {
            ForEach(options) { option in
                Button {
                    selection = option
                } label: {
                    if option == selection {
                        Label(label(option), systemImage: "checkmark")
                    } else {
                        Text(label(option))
                    }
                }
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "line.3.horizontal.decrease.circle")
                    .font(ConsoleType.caption)
                Text(label(selection))
                    .font(ConsoleType.body)
                    .lineLimit(1)
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
            }
            .foregroundStyle(ConsoleTheme.text)
            .padding(.horizontal, 9)
            .frame(height: ConsoleTheme.controlHeight)
            .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 7))
            .overlay(
                RoundedRectangle(cornerRadius: 7).strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(title)
    }
}

// MARK: - 额度条

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

// MARK: - 按钮样式

/// 主按钮。`enabled` 必须参与配色：未接入的按钮要一眼看出点不了。
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
                .font(ConsoleType.body)
                .lineLimit(1)
                .foregroundStyle(active ? Color.white : ConsoleTheme.tertiaryText)
                .padding(.horizontal, 11)
                .frame(height: ConsoleTheme.controlHeight)
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
                .font(ConsoleType.body)
                .lineLimit(1)
                .foregroundStyle(isEnabled ? ConsoleTheme.text : ConsoleTheme.tertiaryText)
                .padding(.horizontal, 11)
                .frame(height: ConsoleTheme.controlHeight)
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

struct ConsoleIconButtonStyle: ButtonStyle {
    var size: CGFloat = ConsoleTheme.controlHeight

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(ConsoleTheme.secondaryText)
            .frame(width: size + 4, height: size)
            .background(
                configuration.isPressed ? ConsoleTheme.cardSubtle : ConsoleTheme.card,
                in: RoundedRectangle(cornerRadius: 7)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 7).strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )
    }
}

// MARK: - 加载

/// 加载态：明确说明正在取什么，但不锁死整个控制台。
struct ConsoleLoadingRow: View {
    let text: String

    var body: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(text)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
        }
        .padding(.vertical, 6)
    }
}

/// 时间戳格式化：`2026-11-09T14:22:31Z` → `11-09 14:22`（本地时区）。
enum ConsoleFormat {
    static func shortTimestamp(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "—" }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime]
        var date = parser.date(from: value)
        if date == nil {
            parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            date = parser.date(from: value)
        }
        guard let date else { return String(value.prefix(10)) }
        return shortDateTime(date)
    }

    static func shortDateTime(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }

    static func relative(_ date: Date?) -> String {
        guard let date else { return "—" }
        let formatter = RelativeDateTimeFormatter()
        formatter.locale = .current
        formatter.unitsStyle = .short
        return formatter.localizedString(for: date, relativeTo: Date())
    }

    /// UID 只露前 8 位：够认人，又不至于把整串带进截图。
    static func maskedUID(_ uid: String) -> String {
        uid.count <= 8 ? uid : String(uid.prefix(8)) + "…"
    }

    static func maskedEmail(_ email: String) -> String {
        guard let at = email.firstIndex(of: "@") else { return maskedUID(email) }
        let name = email[email.startIndex..<at]
        let domain = email[at...]
        guard name.count > 2 else { return "\(name.first ?? "*")***\(domain)" }
        return "\(name.prefix(2))***\(domain)"
    }
}
