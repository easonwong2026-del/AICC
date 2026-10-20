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

struct AICCWidgetView: View {
    let entry: AICCWidgetEntry
    /// Only the offscreen visual checks set this; a real widget always reads
    /// its family from the environment.
    var familyOverride: WidgetFamily? = nil
    @Environment(\.widgetFamily) private var environmentFamily

    private var family: WidgetFamily { familyOverride ?? environmentFamily }

    private var limit: Int {
        entry.configuration.capacity(familyLimit: family == .systemMedium ? 4 : 2)
    }

    private var selection: (shown: [QuotaCard], hidden: Int) {
        entry.snapshot.cards(provider: entry.configuration.provider, limit: limit)
    }

    var body: some View {
        Group {
            if selection.shown.isEmpty {
                EmptyQuotaView(snapshot: entry.snapshot)
            } else if family == .systemMedium {
                MediumQuotaView(entry: entry, cards: selection.shown, hidden: selection.hidden)
            } else {
                SmallQuotaView(entry: entry, cards: selection.shown, hidden: selection.hidden)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .foregroundStyle(Color(nsColor: .labelColor))
        .containerBackground(for: .widget) { Color(nsColor: .windowBackgroundColor) }
        .widgetURL(AICCWidgetMetadata.refreshURL)
    }
}

private extension ShapeStyle where Self == Color {
    static var widgetLabel: Color { Color(nsColor: .labelColor) }
    static var widgetSecondary: Color { Color(nsColor: .secondaryLabelColor) }
    static var widgetCard: Color { Color(nsColor: .controlBackgroundColor) }
    static var widgetEdge: Color { Color(nsColor: .separatorColor) }
}

struct SmallQuotaView: View {
    let entry: AICCWidgetEntry
    let cards: [QuotaCard]
    let hidden: Int

    var body: some View {
        GeometryReader { geometry in
            let spacing: CGFloat = 6
            let footer: CGFloat = hidden > 0 ? 11 : 0
            let each = (geometry.size.height - footer - spacing * CGFloat(max(0, cards.count - 1)))
                / CGFloat(max(1, cards.count))
            VStack(spacing: spacing) {
                ForEach(cards.indices, id: \.self) { index in
                    QuotaCardCell(card: cards[index], compact: true, snapshot: entry.snapshot)
                        .frame(height: each)
                }
                if hidden > 0 {
                    Text("+\(hidden)")
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(.widgetSecondary)
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

    var body: some View {
        GeometryReader { geometry in
            let spacing: CGFloat = 6
            if cards.count == 1 {
                QuotaCardCell(card: cards[0], compact: false, snapshot: entry.snapshot)
            } else {
                let rows = cards.count > 2 ? 2 : 1
                let footer: CGFloat = hidden > 0 ? 11 : 0
                let height = (geometry.size.height - footer - spacing * CGFloat(rows - 1)) / CGFloat(rows)
                VStack(spacing: spacing) {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: spacing), GridItem(.flexible(), spacing: spacing)],
                              spacing: spacing) {
                        ForEach(cards.indices, id: \.self) { index in
                            QuotaCardCell(card: cards[index], compact: rows == 2, snapshot: entry.snapshot)
                                .frame(height: height)
                        }
                    }
                    if hidden > 0 {
                        Text("+\(hidden)")
                            .font(.system(size: 9, weight: .medium))
                            .foregroundStyle(.widgetSecondary)
                            .frame(maxWidth: .infinity, alignment: .trailing)
                    }
                }
            }
        }
    }
}

/// One account's card: provider, an anonymous account label, the first window
/// (or the balance) and at most one more window.  Nothing assumes a five-hour
/// or a weekly window exists, and a balance never becomes a percentage.
struct QuotaCardCell: View {
    let card: QuotaCard
    let compact: Bool
    let snapshot: QuotaSnapshot

    private var status: String? {
        card.statusText ?? (snapshot.stale ? "缓存" : nil)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 1 : 3) {
            HStack(spacing: 4) {
                Text(card.providerName)
                    .font(.system(size: compact ? 9 : 10, weight: .semibold))
                    .foregroundStyle(.widgetLabel)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                Spacer(minLength: 0)
                if let status {
                    Text(status)
                        .font(.system(size: 8))
                        .foregroundStyle(card.failed ? Color.red : Color.orange)
                        .lineLimit(1)
                }
            }
            Text(card.accountLabel)
                .font(.system(size: compact ? 8 : 9))
                .foregroundStyle(.widgetSecondary)
                .lineLimit(1)
            if card.isBalance {
                Text(card.balanceText)
                    .font(.system(size: compact ? 17 : 22, weight: .semibold, design: .rounded).monospacedDigit())
                    .foregroundStyle(.widgetLabel)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                if let second = card.balanceParts.first {
                    Text("\(second.label ?? "") \(second.text)")
                        .font(.system(size: 8).monospacedDigit())
                        .foregroundStyle(.widgetSecondary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                }
            } else if let item = card.primaryItem {
                HStack(alignment: .firstTextBaseline, spacing: 3) {
                    Text(item.primaryText)
                        .font(.system(size: compact ? 16 : 20, weight: .semibold, design: .rounded).monospacedDigit())
                        .foregroundStyle(.widgetLabel)
                        .lineLimit(1)
                        .minimumScaleFactor(0.6)
                    if item.isPercentage {
                        Text(item.name)
                            .font(.system(size: 8))
                            .foregroundStyle(.widgetSecondary)
                            .lineLimit(1)
                    }
                }
                if let second = card.secondaryItem {
                    Text("\(second.name) \(second.primaryText)")
                        .font(.system(size: 8).monospacedDigit())
                        .foregroundStyle(.widgetSecondary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                }
                if let reset = item.reset ?? card.secondaryItem?.reset {
                    Text(reset)
                        .font(.system(size: 8))
                        .foregroundStyle(.widgetSecondary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                } else if card.extraWindows > 0 {
                    Text("+\(card.extraWindows)")
                        .font(.system(size: 8))
                        .foregroundStyle(.widgetSecondary)
                }
            } else {
                Text("无额度")
                    .font(.system(size: compact ? 12 : 14, weight: .medium))
                    .foregroundStyle(.widgetSecondary)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, compact ? 4 : 7)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(.widgetCard, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(.widgetEdge, lineWidth: 0.5))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(card.providerName) \(card.accountLabel) \(card.primaryItem?.primaryText ?? card.balanceText)")
    }
}

struct EmptyQuotaView: View {
    let snapshot: QuotaSnapshot

    private var message: String {
        if snapshot.state == "Empty" { return "Magpie 暂无额度" }
        if !snapshot.available { return "未连接 Magpie" }
        return "暂无额度"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("AICC").font(.system(size: 11, weight: .semibold)).foregroundStyle(.widgetLabel)
            Text(message)
                .font(.system(size: 12))
                .foregroundStyle(.widgetSecondary)
                .lineLimit(2)
                .minimumScaleFactor(0.8)
            Text("打开 Magpie 检查登录")
                .font(.system(size: 9))
                .foregroundStyle(.widgetSecondary)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
