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

    private var quotaData: (primary: Double?, secondary: Double?, isWeekly: Bool, state: String, title: String) {
        if isGoogle {
            let quota = snapshot.google
            let primary = quota?.primary?.remaining
            let secondary = quota?.secondary?.remaining
            let weekly = quota?.isWeekly == true
            let state = snapshot.googleState
            let title = "Google"
            return (primary, secondary, weekly, state, title)
        } else {
            let resolvedAccounts = snapshot.resolvedCodexAccounts
            let primaryAccount = resolvedAccounts?.first

            let primary = resolvedAccounts != nil ? (primaryAccount?.weekly?.remaining ?? primaryAccount?.fiveHour?.remaining) : (snapshot.codexWeeklyRemaining ?? snapshot.codexFiveHourRemaining)
            let secondary = resolvedAccounts != nil ? (primaryAccount?.weekly?.remaining != nil ? primaryAccount?.fiveHour?.remaining : nil) : snapshot.codexSecondaryFiveHourRemaining
            let weekly = resolvedAccounts != nil ? primaryAccount?.weekly?.remaining != nil : snapshot.codexWeeklyRemaining != nil
            let state = primaryAccount?.needsReauth == true ? "reauth" :
                (primary == nil ? "unavailable" : (snapshot.stale || snapshot.codexStale || primaryAccount?.stale == true ? "stale" : "live"))
            let title: String
            if snapshot.codexSelectionMode == "auto" {
                title = isChinese ? "Codex · 自动" : "Codex · Auto"
            } else if let acct = primaryAccount {
                title = "Codex · \(acct.safeDisplayName)"
            } else {
                title = "Codex"
            }
            return (primary, secondary, weekly, state, title)
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
        if !isGoogle, let accounts = snapshot.resolvedCodexAccounts, accounts.count > 1 {
            poolOverviewBody(accounts: accounts)
        } else {
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
                if data.state == "stale" || data.state == "reauth" {
                    Text(data.state == "reauth" ? "Reauth" : (isChinese ? "缓存" : "Cached"))
                        .font(.system(size: 8)).foregroundStyle(.orange)
                }
            }

            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(data.isWeekly ? "W" : "5h").font(.system(size: 9)).foregroundStyle(.secondary)
                quotaNumber(data.primary, size: isGoogle ? 13 : 20)
            }

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
        return VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 3) {
                Text(data.title).font(.system(size: 10, weight: .medium)).foregroundStyle(.secondary)
                    .lineLimit(1).minimumScaleFactor(0.85)
                Spacer(minLength: 0)
                if data.state == "stale" || data.state == "reauth" {
                    Text(data.state == "reauth" ? "Reauth" : (isChinese ? "缓存" : "Cached"))
                        .font(.system(size: 8)).foregroundStyle(.orange)
                }
            }
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(data.isWeekly ? "W" : "5h").font(.system(size: 9)).foregroundStyle(.secondary)
                quotaNumber(data.primary, size: isGoogle ? 13 : 20)
                Spacer(minLength: 0)
                if let secondary = data.secondary {
                    Text("5h " + String(format: "%.0f%%", secondary))
                        .font(.system(size: 9).monospacedDigit()).foregroundStyle(quotaColor(secondary))
                        .fixedSize()
                }
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.primary.opacity(0.1))
                    if let primary = data.primary {
                        Capsule().fill(quotaColor(primary)).frame(width: geo.size.width * min(max(primary, 0), 100) / 100)
                    }
                }
            }
            .frame(height: 3)
            if isGoogle && data.primary == nil {
                Text(isChinese ? "暂无数据" : "No data").font(.system(size: 9)).foregroundStyle(.secondary)
            }
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

    private func poolOverviewBody(accounts: [WidgetCodexAccount]) -> some View {
        let isStale = snapshot.stale || snapshot.codexStale
        let topAccounts = Array(accounts.prefix(2))
        let extraCount = accounts.count - topAccounts.count
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(snapshot.isCodexAutoPool ? (isChinese ? "Codex · 自动" : "Codex · Auto") : "Codex")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                if extraCount > 0 {
                    Text("+\(extraCount)")
                        .font(.system(size: 8.5))
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
                if isStale {
                    Text(isChinese ? "缓存" : "Cached").font(.system(size: 8)).foregroundStyle(.orange)
                }
            }

            ForEach(topAccounts.indices, id: \.self) { i in
                let acct = topAccounts[i]
                let five = acct.fiveHour?.remaining
                let week = acct.weekly?.remaining
                VStack(alignment: .leading, spacing: 1) {
                    HStack(alignment: .firstTextBaseline, spacing: 3) {
                        Text(acct.safeDisplayName)
                            .font(.system(size: 9.5, weight: .semibold))
                            .lineLimit(1)
                        Spacer(minLength: 0)
                        if acct.needsReauth == true {
                            Text("Reauth").font(.system(size: 8.5)).foregroundStyle(.orange)
                        } else {
                            if acct.stale == true && !isStale {
                                Text(isChinese ? "缓存" : "Cached").font(.system(size: 8)).foregroundStyle(.orange)
                            }
                            if snapshot.codexSelectionMode != "auto" &&
                                (snapshot.codexActiveAccountID.map { $0 == acct.id } ?? (acct.active == true)) {
                                Circle().fill(Color.green).frame(width: 4, height: 4)
                                    .accessibilityLabel("Active")
                            }
                            HStack(spacing: 4) {
                                Text("W" + (week.map { String(format: "%.0f", $0) } ?? "—"))
                                    .foregroundStyle(quotaColor(week))
                                Text("5h" + (five.map { String(format: "%.0f", $0) } ?? "—"))
                                    .foregroundStyle(quotaColor(five))
                            }
                            .font(.system(size: 11, weight: .semibold).monospacedDigit())
                            .fixedSize()
                        }
                    }
                    GeometryReader { geo in
                        ZStack(alignment: .leading) {
                            Capsule().fill(Color.primary.opacity(0.08))
                            if let w = week {
                                Capsule().fill(quotaColor(w)).frame(width: geo.size.width * min(max(w, 0), 100) / 100)
                            }
                        }
                    }
                    .frame(height: 2.5)
                }
                .padding(.top, i > 0 ? 1 : 0)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .combine)
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

    private var gridBody: some View { compactBody }

    private var compactBody: some View {
        let data = balanceData
        let status = metric == .workbuddy ? snapshot.workbuddyCompactStatusText : snapshot.deepseekCompactStatusText
        return VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 4) {
                Text(data.title).font(.system(size: 10, weight: .medium)).foregroundStyle(.secondary)
                Spacer(minLength: 0)
                Circle().fill(data.state == "live" && status.isEmpty ? Color.green : Color.orange)
                    .frame(width: 4, height: 4)
            }
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(data.valueText)
                    .font(.system(size: 12, weight: .semibold, design: .rounded).monospacedDigit())
                    .lineLimit(1).minimumScaleFactor(0.7)
                Text(data.unit).font(.system(size: 9)).foregroundStyle(.secondary).fixedSize()
            }
            if !status.isEmpty {
                Text(status == "Cached" ? (isChinese ? "缓存" : "Cached") :
                    (status == "Unavailable" ? (isChinese ? "不可用" : "Unavailable") : status))
                    .font(.system(size: 8)).foregroundStyle(.orange).lineLimit(2)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .combine)
    }

    private var spaciousBody: some View { compactBody }
    private var compactRowBody: some View { compactBody }

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
