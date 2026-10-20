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
/// resolve to the same colour as a custom fill.  Owning both the background
/// and the ink keeps every state readable, in light and dark alike.
struct WidgetPalette {
    let background: Color
    let card: Color
    let edge: Color
    let ink: Color
    let muted: Color
    let warning: Color

    static func of(_ scheme: ColorScheme) -> WidgetPalette {
        scheme == .dark
            ? WidgetPalette(background: Color(red: 0.09, green: 0.09, blue: 0.10),
                            card: Color(red: 0.15, green: 0.15, blue: 0.17),
                            edge: Color(.sRGB, white: 1, opacity: 0.10),
                            ink: Color(.sRGB, white: 0.98, opacity: 1),
                            muted: Color(.sRGB, white: 0.68, opacity: 1),
                            warning: Color(red: 1.0, green: 0.68, blue: 0.30))
            : WidgetPalette(background: Color(red: 0.95, green: 0.95, blue: 0.93),
                            card: Color.white,
                            edge: Color(.sRGB, white: 0, opacity: 0.08),
                            ink: Color(red: 0.07, green: 0.07, blue: 0.08),
                            muted: Color(red: 0.42, green: 0.42, blue: 0.44),
                            warning: Color(red: 0.72, green: 0.37, blue: 0.02))
    }
}

struct AICCWidgetView: View {
    let entry: AICCWidgetEntry
    /// Only the offscreen visual checks set this; a real widget reads the family
    /// from the environment.
    var familyOverride: WidgetFamily? = nil
    @Environment(\.widgetFamily) private var environmentFamily
    @Environment(\.colorScheme) private var scheme

    private var family: WidgetFamily { familyOverride ?? environmentFamily }
    private var palette: WidgetPalette { .of(scheme) }

    private var limit: Int {
        entry.configuration.capacity(familyLimit: family == .systemMedium ? 4 : 2)
    }

    private var selection: (shown: [QuotaCard], hidden: Int) {
        entry.snapshot.cards(provider: entry.configuration.provider, limit: limit)
    }

    var body: some View {
        Group {
            if selection.shown.isEmpty {
                EmptyQuotaView(snapshot: entry.snapshot, palette: palette)
            } else if family == .systemMedium {
                MediumQuotaView(entry: entry, cards: selection.shown, hidden: selection.hidden, palette: palette)
            } else {
                SmallQuotaView(entry: entry, cards: selection.shown, hidden: selection.hidden, palette: palette)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .containerBackground(for: .widget) { palette.background }
    }
}

struct SmallQuotaView: View {
    let entry: AICCWidgetEntry
    let cards: [QuotaCard]
    let hidden: Int
    let palette: WidgetPalette

    var body: some View {
        GeometryReader { geometry in
            let spacing: CGFloat = 6
            let footer: CGFloat = hidden > 0 ? 12 : 0
            let each = (geometry.size.height - footer - spacing * CGFloat(max(0, cards.count - 1)))
                / CGFloat(max(1, cards.count))
            VStack(spacing: spacing) {
                ForEach(cards.indices, id: \.self) { index in
                    QuotaCardCell(card: cards[index], compact: true, palette: palette)
                        .frame(height: max(44, each))
                }
                if hidden > 0 {
                    Text("+\(hidden)")
                        .font(.system(size: 8, weight: .medium).monospacedDigit())
                        .foregroundStyle(palette.muted)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
            }
        }
    }
}

struct MediumQuotaView: View {
    let entry: AICCWidgetEntry
    let cards: [QuotaCard]
    let hidden: Int
    let palette: WidgetPalette

    var body: some View {
        GeometryReader { geometry in
            let spacing: CGFloat = 6
            let footer: CGFloat = hidden > 0 ? 12 : 0
            let usable = geometry.size.height - footer
            if cards.count == 1 {
                QuotaCardCell(card: cards[0], compact: false, palette: palette, hero: true)
                    .frame(height: usable)
            } else {
                let rows = cards.count > 2 ? 2 : 1
                let height = (usable - spacing * CGFloat(rows - 1)) / CGFloat(rows)
                VStack(spacing: spacing) {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: spacing),
                                        GridItem(.flexible(), spacing: spacing)],
                              spacing: spacing) {
                        ForEach(cards.indices, id: \.self) { index in
                            QuotaCardCell(card: cards[index], compact: rows == 2, palette: palette)
                                .frame(height: max(44, height))
                        }
                    }
                    if hidden > 0 {
                        Text("还有 \(hidden) 张")
                            .font(.system(size: 8, weight: .medium))
                            .foregroundStyle(palette.muted)
                            .frame(maxWidth: .infinity, alignment: .trailing)
                    }
                }
            }
        }
    }
}

/// One provider account: header (provider, plan or status), the anonymous
/// account label, the leading window with its share bar, then either the second
/// window or the balance breakdown.  Numbers are monospaced so a refresh never
/// makes them jitter.
struct QuotaCardCell: View {
    let card: QuotaCard
    let compact: Bool
    let palette: WidgetPalette
    /// A medium widget showing one account gets a larger, vertically centred
    /// card instead of a small one floating at the top of the tile.
    var hero: Bool = false

    private var status: (text: String, color: Color)? {
        if card.failed { return ("读取失败", palette.warning) }
        if card.deferred { return ("本轮跳过", palette.warning) }
        if card.stale { return ("缓存", palette.warning) }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 2 : 4) {
            if hero { Spacer(minLength: 0) }
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(card.providerName)
                    .font(.system(size: hero ? 13 : compact ? 9.5 : 10.5, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Spacer(minLength: 2)
                if let status {
                    Text(status.text)
                        .font(.system(size: hero ? 9.5 : 8, weight: .semibold))
                        .foregroundStyle(status.color)
                        .lineLimit(1)
                } else if let plan = card.plan, !plan.isEmpty {
                    Text(plan)
                        .font(.system(size: 8, weight: .medium))
                        .foregroundStyle(palette.muted)
                        .lineLimit(1)
                }
            }

            Text(card.accountLabel)
                .font(.system(size: hero ? 10 : 8.5))
                .foregroundStyle(palette.muted)
                .lineLimit(1)

            if card.isBalance {
                balance(compact: compact)
            } else if let item = card.primaryItem {
                window(item, compact: compact)
                if let second = card.secondaryItem {
                    secondary(second)
                } else if let reset = item.reset {
                    Text(reset)
                        .font(.system(size: 8))
                        .foregroundStyle(palette.muted)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                }
            } else {
                Text("无额度")
                    .font(.system(size: compact ? 11 : 13, weight: .medium))
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
        .accessibilityLabel("\(card.providerName) \(card.accountLabel) \(card.primaryItem?.primaryText ?? card.balanceText)")
    }

    private func window(_ item: QuotaItem, compact: Bool) -> some View {
        VStack(alignment: .leading, spacing: compact ? 2 : 4) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(item.primaryText)
                    .font(.system(size: hero ? 34 : compact ? 18 : 22, weight: .semibold, design: .rounded).monospacedDigit())
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                    .minimumScaleFactor(0.55)
                Text(item.isPercentage ? item.name : (item.unit ?? item.name))
                    .font(.system(size: hero ? 11 : 8.5))
                    .foregroundStyle(palette.muted)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            if item.isPercentage, let remaining = item.remaining {
                ShareBar(share: remaining, palette: palette)
            }
        }
    }

    private func secondary(_ item: QuotaItem) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 3) {
            Text(item.name)
                .font(.system(size: hero ? 10 : 8))
                .foregroundStyle(palette.muted)
                .lineLimit(1)
            Text(item.primaryText)
                .font(.system(size: hero ? 11 : 8.5, weight: .medium).monospacedDigit())
                .foregroundStyle(palette.ink.opacity(0.85))
                .lineLimit(1)
            Spacer(minLength: 0)
            if let text = WidgetClock.short(item.resetsAt) ?? item.reset {
                Text(text)
                    .font(.system(size: 7.5).monospacedDigit())
                    .foregroundStyle(palette.muted)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
        }
    }

    private func balance(compact: Bool) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(card.balanceText)
                .font(.system(size: hero ? 34 : compact ? 18 : 22, weight: .semibold, design: .rounded).monospacedDigit())
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .minimumScaleFactor(0.55)
            if let first = card.balanceParts.first {
                Text([first.label, first.text].compactMap { $0 }.joined(separator: " "))
                    .font(.system(size: 8))
                    .foregroundStyle(palette.muted)
                    .lineLimit(1)
            }
        }
    }
}

/// Reset times are the tightest text in a small widget: "23:39" today,
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

/// A thin share bar for percentage windows; silent when the share is unknown.
struct ShareBar: View {
    let share: Double
    let palette: WidgetPalette

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(palette.ink.opacity(0.08))
                Capsule()
                    .fill(palette.ink.opacity(0.42))
                    .frame(width: max(2, geometry.size.width * min(1, max(0, share / 100))))
            }
        }
        .frame(height: 2.5)
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
