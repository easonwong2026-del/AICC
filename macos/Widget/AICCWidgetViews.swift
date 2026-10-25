import SwiftUI
import WidgetKit
import AppKit

struct AICCWidgetEntry: TimelineEntry {
    let date: Date
    let snapshot: QuotaSnapshot
    let configuration: AICCWidgetConfiguration

    init(date: Date = .now, snapshot: QuotaSnapshot, configuration: AICCWidgetConfiguration = AICCWidgetConfiguration()) {
        self.date = date
        self.snapshot = snapshot
        self.configuration = configuration
    }
}

/// Explicit colours instead of semantic ones: a desktop widget is composited
/// with the wallpaper's vibrant material, where `.primary`/`.secondary` can
/// resolve to the same colour as a custom fill.
struct WidgetPalette {
    let background: Color
    let card: Color
    let edge: Color
    let ink: Color
    let muted: Color
    let warning: Color
    /// True when the system is rendering us in accented mode: colours are gone.
    var accented: Bool = false
    var isDark: Bool = true

    /// `accented` is what macOS 26 does to desktop widgets whenever another app
    /// is in front ("Dim widgets on desktop" = Automatically): the system tints
    /// every colour to white and drops the background, so only **opacity**
    /// survives.  An opaque card fill therefore turns into a solid white block
    /// that swallows the equally white text — the white-boxes state.  There the
    /// card has to be a soft white wash instead, and the text stays opaque.
    static func of(_ scheme: ColorScheme, mode: WidgetRenderingMode = .fullColor) -> WidgetPalette {
        if mode != .fullColor {
            return WidgetPalette(background: .clear,
                                 card: Color(.sRGB, white: 1, opacity: 0.14),
                                 edge: Color(.sRGB, white: 1, opacity: 0.26),
                                 ink: Color(.sRGB, white: 1, opacity: 1),
                                 muted: Color(.sRGB, white: 1, opacity: 0.66),
                                 warning: Color(.sRGB, white: 1, opacity: 0.92),
                                 accented: true,
                                 isDark: true)
        }
        return scheme == .dark
            ? WidgetPalette(background: Color(red: 0.09, green: 0.09, blue: 0.10),
                            card: Color(red: 0.15, green: 0.15, blue: 0.17),
                            edge: Color(.sRGB, white: 1, opacity: 0.10),
                            ink: Color(.sRGB, white: 0.98, opacity: 1),
                            muted: Color(.sRGB, white: 0.68, opacity: 1),
                            warning: Color(red: 1.0, green: 0.62, blue: 0.28),
                            isDark: true)
            : WidgetPalette(background: Color(red: 0.95, green: 0.95, blue: 0.93),
                            card: Color.white,
                            edge: Color(.sRGB, white: 0, opacity: 0.08),
                            ink: Color(red: 0.07, green: 0.07, blue: 0.08),
                            muted: Color(red: 0.42, green: 0.42, blue: 0.44),
                            warning: Color(red: 0.72, green: 0.37, blue: 0.02),
                            isDark: false)
    }

    // MARK: - Provider colour

    /// One hue per provider, so four cards are four colours instead of four grey
    /// bars.  Known providers get a hand-picked hue; anything else gets a stable
    /// hue derived from its id, so a new Magpie plugin is never colourless.
    private static let knownHues: [String: Double] = [
        "codex": 0.57,          // blue
        "workbuddy": 0.40,      // green
        "antigravity": 0.74,    // violet
        "zcode": 0.68, "zhipu": 0.68, "glm": 0.68,
        "kiro": 0.08,           // amber
        "deepseek": 0.62,
        "minimax": 0.02,        // red
        "gemini": 0.60,
        "claude": 0.09,
        "commandcode": 0.52,
        "cursor": 0.55,
        "copilot": 0.45,
    ]

    func accent(for provider: String) -> Color {
        // Accented rendering tints everything white; a hue would be thrown away.
        if accented { return ink }
        let key = provider.lowercased()
        let hue = Self.knownHues[key]
            ?? Self.knownHues.first { key.hasPrefix($0.key) }?.value
            ?? Self.hue(for: key)
        return Color(hue: hue, saturation: isDark ? 0.58 : 0.74, brightness: isDark ? 1.0 : 0.66)
    }

    /// Deterministic hue for an unknown provider id.
    private static func hue(for provider: String) -> Double {
        var hash: UInt64 = 5381
        for byte in provider.utf8 { hash = hash &* 33 &+ UInt64(byte) }
        return Double(hash % 360) / 360
    }

    /// A window that is nearly used up turns amber, whatever its provider.
    func tone(for provider: String, remaining: Double?) -> Color {
        if !accented, let remaining, remaining <= 15 { return warning }
        return accent(for: provider)
    }
}

struct AICCWidgetView: View {
    let entry: AICCWidgetEntry
    /// Only the offscreen visual checks set this; a real widget reads the family
    /// from the environment.
    var familyOverride: WidgetFamily? = nil
    @Environment(\.widgetFamily) private var environmentFamily
    @Environment(\.colorScheme) private var scheme
    @Environment(\.widgetRenderingMode) private var renderingMode

    private var family: WidgetFamily { familyOverride ?? environmentFamily }
    private var palette: WidgetPalette { .of(scheme, mode: renderingMode) }

    private var limit: Int {
        entry.configuration.capacity(familyLimit: family == .systemMedium ? 4 : 2)
    }

    private var selection: (shown: [QuotaSlot], hidden: Int) {
        entry.snapshot.slots(selection: entry.configuration.selection,
                             provider: entry.configuration.provider,
                             limit: limit)
    }

    var body: some View {
        Group {
            if selection.shown.isEmpty {
                EmptyQuotaView(snapshot: entry.snapshot, palette: palette)
            } else if family == .systemMedium {
                MediumQuotaView(entry: entry, slots: selection.shown, hidden: selection.hidden, palette: palette)
            } else {
                SmallQuotaView(entry: entry, slots: selection.shown, hidden: selection.hidden, palette: palette)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .containerBackground(for: .widget) { palette.background }
    }
}

struct SmallQuotaView: View {
    let entry: AICCWidgetEntry
    let slots: [QuotaSlot]
    let hidden: Int
    let palette: WidgetPalette

    var body: some View {
        GeometryReader { geometry in
            let spacing: CGFloat = 6
            let footer: CGFloat = hidden > 0 ? 11 : 0
            let each = (geometry.size.height - footer - spacing * CGFloat(max(0, slots.count - 1)))
                / CGFloat(max(1, slots.count))
            VStack(spacing: spacing) {
                ForEach(slots.indices, id: \.self) { index in
                    QuotaCardCell(slot: slots[index], compact: true, palette: palette)
                        .frame(height: max(44, each))
                }
                if hidden > 0 {
                    Text("+\(hidden)")
                        .font(.system(size: 9, weight: .medium).monospacedDigit())
                        .foregroundStyle(palette.muted)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
            }
        }
    }
}

struct MediumQuotaView: View {
    let entry: AICCWidgetEntry
    let slots: [QuotaSlot]
    let hidden: Int
    let palette: WidgetPalette

    var body: some View {
        GeometryReader { geometry in
            let spacing: CGFloat = 6
            let footer: CGFloat = hidden > 0 ? 11 : 0
            let usable = geometry.size.height - footer
            if slots.count == 1 {
                QuotaCardCell(slot: slots[0], compact: false, palette: palette, hero: true)
                    .frame(height: usable)
            } else {
                let rows = slots.count > 2 ? 2 : 1
                let height = (usable - spacing * CGFloat(rows - 1)) / CGFloat(rows)
                VStack(spacing: spacing) {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: spacing),
                                        GridItem(.flexible(), spacing: spacing)],
                              spacing: spacing) {
                        ForEach(slots.indices, id: \.self) { index in
                            QuotaCardCell(slot: slots[index], compact: rows == 2, palette: palette)
                                .frame(height: max(44, height))
                        }
                    }
                    if hidden > 0 {
                        Text("还有 \(hidden) 张")
                            .font(.system(size: 9, weight: .medium))
                            .foregroundStyle(palette.muted)
                            .frame(maxWidth: .infinity, alignment: .trailing)
                    }
                }
            }
        }
    }
}

/// One account card: provider and masked account on the title line, what is
/// left in the leading window, the second window beside it, then the share bar
/// and the reset time.  Three rows instead of five, so two cards in a medium
/// widget stay aligned.
struct QuotaCardCell: View {
    let slot: QuotaSlot
    let compact: Bool
    let palette: WidgetPalette
    var hero: Bool = false

    private var card: QuotaCard { slot.card }
    private var item: QuotaItem { slot.primary }

    /// The provider's own hue, ambered once the window is nearly used up.
    private var tone: Color { palette.tone(for: card.provider, remaining: item.remaining) }

    private var status: (text: String, color: Color)? {
        if card.failed { return ("读取失败", palette.warning) }
        if card.deferred { return ("本轮跳过", palette.warning) }
        if card.stale { return ("缓存", palette.warning) }
        return nil
    }

    private var unitLabel: String? {
        if item.unlimited { return nil }
        if item.isPercentage { return item.name }
        if let unit = item.unit, !unit.isEmpty { return WidgetUnit.localized(unit) }
        return item.name
    }

    private var trailingDetail: String? {
        guard let second = slot.secondary else { return item.totalText }
        // The small widget has ~55pt for this: "7 days 74%" truncates, so the
        // window name is compressed the way a person would write it.
        let name = compact ? WidgetWindow.short(second.name) : second.name
        return "\(name) \(second.primaryText)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 2 : 4) {
            if hero { Spacer(minLength: 0) }

            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(card.title)
                    .font(.system(size: hero ? 13.5 : compact ? 10 : 11, weight: .semibold))
                    .foregroundStyle(palette.accent(for: card.provider))
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                Spacer(minLength: 2)
                if let status {
                    Text(status.text)
                        .font(.system(size: hero ? 10 : 9, weight: .semibold))
                        .foregroundStyle(status.color)
                        .lineLimit(1)
                } else if let plan = card.plan, !plan.isEmpty {
                    Text(plan)
                        .font(.system(size: hero ? 10 : 9, weight: .medium))
                        .foregroundStyle(palette.muted)
                        .lineLimit(1)
                }
            }

            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(item.primaryText)
                    .font(.system(size: hero ? 34 : compact ? 19 : 22, weight: .semibold, design: .rounded).monospacedDigit())
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                    .minimumScaleFactor(0.55)
                if let unitLabel {
                    Text(unitLabel)
                        .font(.system(size: hero ? 11.5 : compact ? 9 : 9.5))
                        .minimumScaleFactor(0.9)
                        .foregroundStyle(palette.muted)
                        .lineLimit(1)
                }
                Spacer(minLength: 2)
                if let trailingDetail {
                    Text(trailingDetail)
                        .font(.system(size: hero ? 11 : compact ? 9 : 9.5).monospacedDigit())
                        .foregroundStyle(palette.muted)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                }
            }

            if let share = item.remaining, !item.unlimited {
                ShareBar(share: share, tone: tone, track: palette.ink.opacity(0.12))
            }

            if let reset = WidgetClock.short(item.resetsAt) ?? WidgetClock.short(slot.secondary?.resetsAt) {
                Text(reset)
                    .font(.system(size: hero ? 10.5 : 9).monospacedDigit())
                    .foregroundStyle(palette.muted)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
            } else if slot.extra > 0 {
                Text("+\(slot.extra) 个窗口")
                    .font(.system(size: 9))
                    .foregroundStyle(palette.muted)
            }

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 9)
        .padding(.top, compact ? 5 : 7)
        .padding(.bottom, 5)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
            .strokeBorder(palette.edge, lineWidth: 0.5))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(card.title) \(item.name) \(item.primaryText)")
    }
}

/// Window names come from the vendor ("5 hours", "7 days", "Credits").  In the
/// small widget they have to fit in a few characters, so the common ones get a
/// short form and everything else is shortened without losing its identity.
enum WidgetWindow {
    static func short(_ name: String) -> String {
        let lower = name.lowercased()
        if let count = leadingNumber(lower) {
            if lower.contains("hour") { return "\(count)时" }
            if lower.contains("day") { return "\(count)天" }
            if lower.contains("week") { return "\(count)周" }
            if lower.contains("month") { return "\(count)月" }
            if lower.contains("min") { return "\(count)分" }
        }
        if lower.contains("hour") { return "小时" }
        if lower.contains("day") { return "天" }
        if lower.contains("week") { return "周" }
        return name.count > 6 ? String(name.prefix(5)) + "…" : name
    }

    private static func leadingNumber(_ text: String) -> String? {
        let digits = text.prefix { $0.isNumber || $0 == "." }
        return digits.isEmpty ? nil : String(digits)
    }
}

/// Magpie reports units in English ("credits", "prompts"); the panel is Chinese.
enum WidgetUnit {    static func localized(_ unit: String) -> String {
        switch unit.lowercased() {
        case "credits", "credit": return "积分"
        case "prompts", "prompt", "requests", "request", "calls": return "次"
        case "tokens", "token": return "token"
        default: return unit
        }
    }
}

/// A thin share bar for percentage windows; silent when the share is unknown.
struct ShareBar: View {
    let share: Double
    let tone: Color
    let track: Color

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(track)
                Capsule()
                    .fill(tone)
                    .frame(width: max(2, geometry.size.width * min(1, max(0, share / 100))))
            }
        }
        .frame(height: 2.5)
    }
}

/// Reset times are the tightest text in a widget: "23:39" today,
/// "明天 09:00", "周三 14:30" this week, else "10-27 14:30".
enum WidgetClock {
    static func short(_ epoch: Double?, now: Date = .now) -> String? {
        guard let epoch else { return nil }
        let date = Date(timeIntervalSince1970: epoch)
        let calendar = Calendar.current
        let time = DateFormatter()
        time.locale = .current
        time.dateFormat = "HH:mm"
        if calendar.isDate(date, inSameDayAs: now) { return time.string(from: date) }
        if let tomorrow = calendar.date(byAdding: .day, value: 1, to: now),
           calendar.isDate(date, inSameDayAs: tomorrow) {
            return "明天 " + time.string(from: date)
        }
        let days = calendar.dateComponents([.day], from: now, to: date).day ?? 99
        if days > 0, days < 7 {
            let weekday = DateFormatter()
            weekday.locale = .current
            weekday.dateFormat = "EEE"
            return weekday.string(from: date) + " " + time.string(from: date)
        }
        let stamped = DateFormatter()
        stamped.locale = .current
        stamped.dateFormat = "MM-dd HH:mm"
        return stamped.string(from: date)
    }
}

struct EmptyQuotaView: View {
    let snapshot: QuotaSnapshot
    let palette: WidgetPalette

    private var title: String {
        if snapshot.state == "Empty" { return "Magpie 暂无额度" }
        if !snapshot.available { return "未连接 Magpie" }
        return "暂无额度"
    }

    private var detail: String {
        snapshot.state == "Empty" ? "在 Magpie 登录服务商后自动出现" : "确认 Magpie 正在运行"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Spacer(minLength: 0)
            HStack(spacing: 6) {
                Image(systemName: "rectangle.3.group")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.muted)
                Text("AICC")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.ink)
                Spacer(minLength: 0)
            }
            Text(title)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(palette.ink)
                .lineLimit(2)
                .minimumScaleFactor(0.8)
            Text(detail)
                .font(.system(size: 9))
                .foregroundStyle(palette.muted)
                .lineLimit(2)
                .minimumScaleFactor(0.8)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(2)
    }
}
