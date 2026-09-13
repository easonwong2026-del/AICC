import SwiftUI
import WidgetKit
import AppIntents

struct AICCWidgetEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetDisplaySnapshot
    let configuration: AICCWidgetConfigurationIntent

    init(date: Date = .now, snapshot: WidgetDisplaySnapshot, configuration: AICCWidgetConfigurationIntent = AICCWidgetConfigurationIntent()) {
        self.date = date
        self.snapshot = snapshot
        self.configuration = configuration
    }
}

enum MetricCardLayout: Sendable {
    case compact
    case grid
    case spacious
    case compactRow
}

struct MetricCardView: View {
    let metric: WidgetMetricOption
    let snapshot: WidgetDisplaySnapshot
    let layout: MetricCardLayout
    let isChinese: Bool

    var body: some View {
        if metric.isQuota {
            QuotaCardView(metric: metric, snapshot: snapshot, layout: layout, isChinese: isChinese)
        } else {
            BalanceCardView(metric: metric, snapshot: snapshot, layout: layout, isChinese: isChinese)
        }
    }
}

struct QuotaCardView: View {
    let metric: WidgetMetricOption
    let snapshot: WidgetDisplaySnapshot
    let layout: MetricCardLayout
    let isChinese: Bool

    private var isGoogle: Bool { metric == .google }

    private var quotaData: (primary: Double?, secondary: Double?, isWeekly: Bool, state: String, title: String, reset: String?) {
        if isGoogle {
            let quota = snapshot.google
            let primary = quota?.primary?.remaining
            let secondary = quota?.secondary?.remaining
            let weekly = quota?.isWeekly == true
            let state = snapshot.googleState
            let title = "Google" + (primary == nil ? "" : (weekly ? (isChinese ? " 周" : " Weekly") : " 5h"))
            let reset = quota?.reset.map { String($0.dropFirst($0.count >= 16 ? 5 : 0)) }
            return (primary, secondary, weekly, state, title, reset)
        } else {
            let primary = snapshot.codexWeeklyRemaining ?? snapshot.codexFiveHourRemaining
            let secondary = snapshot.codexSecondaryFiveHourRemaining
            let weekly = snapshot.codexWeeklyRemaining != nil
            let state = snapshot.codexState
            let title = "Codex" + (primary == nil ? "" : (weekly ? (isChinese ? " 周" : " Weekly") : " 5h"))
            let reset = snapshot.codexResetShortText?.replacingOccurrences(of: "重置于 ", with: "")
            return (primary, secondary, weekly, state, title, reset)
        }
    }

    private func quotaNumber(_ value: Double?, size: CGFloat) -> some View {
        HStack(alignment: .lastTextBaseline, spacing: 1) {
            Text(value.map { String(format: "%.0f", $0) } ?? "—")
                .font(.system(size: size, weight: .bold, design: .rounded).monospacedDigit())
            if value != nil { Text("%").font(.system(size: size / 2, weight: .medium)) }
        }
        .foregroundStyle(quotaColor(value))
        .lineLimit(1)
    }

    private func quotaColor(_ value: Double?) -> Color {
        guard let value else { return .secondary }
        return value > 70 ? .green : (value >= 30 ? .yellow : .red)
    }

    var body: some View {
        switch layout {
        case .compact:
            compactBody
        case .grid:
            gridBody
        case .spacious:
            spaciousBody
        case .compactRow:
            compactRowBody
        }
    }

    private var gridBody: some View {
        let data = quotaData
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(data.title)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
                Spacer(minLength: 0)
                if data.state == "stale" {
                    Text(isChinese ? "缓存" : "Cached").font(.system(size: 8)).foregroundStyle(.orange)
                }
            }

            quotaNumber(data.primary, size: 20)

            if let primary = data.primary {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.primary.opacity(0.1))
                        Capsule().fill(quotaColor(primary)).frame(width: geo.size.width * min(max(primary, 0), 100) / 100)
                    }
                }
                .frame(height: 3)
            } else {
                Capsule().fill(Color.primary.opacity(0.1)).frame(height: 3)
            }

            HStack(spacing: 3) {
                if let secondary = data.secondary {
                    Text("5h").foregroundStyle(.secondary)
                    Text(String(format: "%.0f%%", secondary)).foregroundStyle(quotaColor(secondary))
                } else if isGoogle && (!data.isWeekly || data.primary == nil) {
                    Text(data.primary == nil ? (isChinese ? "暂无数据" : "No data") : (isChinese ? "周额度暂无数据" : "Weekly unavailable"))
                        .foregroundStyle(.secondary)
                } else if let reset = data.reset {
                    Text(reset).foregroundStyle(.secondary)
                }
            }
            .font(.system(size: 9))
            .lineLimit(1)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .combine)
    }

    private var compactBody: some View {
        let data = quotaData
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .lastTextBaseline, spacing: 3) {
                Text(data.title)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
                quotaNumber(data.primary, size: 19)
            }
            .lineLimit(1)

            if let primary = data.primary {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.primary.opacity(0.1))
                        Capsule().fill(quotaColor(primary)).frame(width: geo.size.width * min(max(primary, 0), 100) / 100)
                    }
                }
                .frame(height: 3)
            }

            HStack(spacing: 2) {
                if let secondary = data.secondary {
                    Text("5h").foregroundStyle(.secondary)
                    Text(String(format: "%.0f%%", secondary)).foregroundStyle(quotaColor(secondary))
                } else if isGoogle && (!data.isWeekly || data.primary == nil) {
                    Text(data.primary == nil ? (isChinese ? "暂无数据" : "No data") : (isChinese ? "周额度暂无数据" : "Weekly unavailable"))
                        .foregroundStyle(.secondary)
                } else if let reset = data.reset {
                    Text(reset).foregroundStyle(.secondary)
                }
                if data.state == "stale" {
                    Spacer(minLength: 0)
                    Text(isChinese ? "缓存" : "Cached").foregroundStyle(.orange)
                }
            }
            .font(.system(size: 9))
            .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private var spaciousBody: some View {
        gridBody
    }

    private var compactRowBody: some View {
        gridBody
    }
}

struct BalanceCardView: View {
    let metric: WidgetMetricOption
    let snapshot: WidgetDisplaySnapshot
    let layout: MetricCardLayout
    let isChinese: Bool

    private var balanceData: (title: String, valueText: String, unit: String, state: String) {
        if metric == .workbuddy {
            return (
                "WorkBuddy",
                snapshot.workbuddyPointsText,
                isChinese ? "积分" : "pts",
                snapshot.workbuddyState
            )
        } else {
            return (
                "DeepSeek",
                snapshot.deepseekBalanceText,
                snapshot.deepseekCurrency,
                snapshot.deepseekState
            )
        }
    }

    var body: some View {
        switch layout {
        case .compact:
            compactBody
        case .grid:
            gridBody
        case .spacious:
            spaciousBody
        case .compactRow:
            compactRowBody
        }
    }

    private var gridBody: some View {
        let data = balanceData
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(data.title)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
                Spacer(minLength: 0)
                if data.state == "stale" {
                    Text(isChinese ? "缓存" : "Cached").font(.system(size: 8)).foregroundStyle(.orange)
                }
            }

            Text(data.valueText)
                .font(.system(size: 20, weight: .bold, design: .rounded).monospacedDigit())
                .lineLimit(1)
                .minimumScaleFactor(0.7)

            Capsule()
                .fill(Color.primary.opacity(0.08))
                .frame(height: 3)

            Text(data.unit)
                .font(.system(size: 9))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .combine)
    }

    private var compactBody: some View {
        let data = balanceData
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .lastTextBaseline, spacing: 3) {
                Text(data.title)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
                Text(data.valueText)
                    .font(.system(size: 19, weight: .bold, design: .rounded).monospacedDigit())
                    .lineLimit(1)
                    .minimumScaleFactor(0.55)
            }
            .lineLimit(1)

            Capsule()
                .fill(Color.primary.opacity(0.08))
                .frame(height: 3)

            HStack(spacing: 2) {
                Text(data.unit)
                    .foregroundStyle(.secondary)
                if data.state == "stale" {
                    Spacer(minLength: 0)
                    Text(isChinese ? "缓存" : "Cached").foregroundStyle(.orange)
                }
            }
            .font(.system(size: 9))
            .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private var spaciousBody: some View {
        gridBody
    }

    private var compactRowBody: some View {
        gridBody
    }
}

struct AICCWidgetView: View {
    @Environment(\.locale) private var locale
    @Environment(\.widgetFamily) private var family
    var familyOverride: WidgetFamily? = nil

    let entry: AICCWidgetEntry

    private var isChinese: Bool {
        locale.identifier.hasPrefix("zh")
    }

    private var effectiveFamily: WidgetFamily {
        familyOverride ?? family
    }

    var body: some View {
        Group {
            if effectiveFamily == .systemMedium {
                mediumContent
            } else {
                smallContent
            }
        }
        .containerBackground(.fill.tertiary, for: .widget)
    }

    private var refreshButton: some View {
        Button(intent: RefreshWidgetIntent()) {
            Image(systemName: "arrow.clockwise")
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(.secondary)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Refresh Widget")
    }

    private var mediumContent: some View {
        let metrics = entry.configuration.resolvedMetrics(for: .systemMedium)
        return VStack(spacing: 5) {
            HStack {
                Text("AICC").font(.system(size: 11, weight: .bold))
                Spacer()
                refreshButton
            }
            HStack(spacing: 12) {
                MetricCardView(metric: metrics[0], snapshot: entry.snapshot, layout: .grid, isChinese: isChinese)
                Divider()
                MetricCardView(metric: metrics[1], snapshot: entry.snapshot, layout: .grid, isChinese: isChinese)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            HStack(spacing: 12) {
                MetricCardView(metric: metrics[2], snapshot: entry.snapshot, layout: .grid, isChinese: isChinese)
                Divider()
                MetricCardView(metric: metrics[3], snapshot: entry.snapshot, layout: .grid, isChinese: isChinese)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private var smallContent: some View {
        let metrics = entry.configuration.resolvedMetrics(for: .systemSmall)
        return VStack(spacing: 5) {
            HStack {
                Text("AICC").font(.system(size: 11, weight: .bold))
                Spacer()
                refreshButton
            }
            MetricCardView(metric: metrics[0], snapshot: entry.snapshot, layout: .compact, isChinese: isChinese)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            MetricCardView(metric: metrics[1], snapshot: entry.snapshot, layout: .compact, isChinese: isChinese)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(11)
    }
}
