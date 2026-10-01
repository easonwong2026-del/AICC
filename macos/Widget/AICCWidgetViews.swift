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

struct MetricCardView: View {
    let metric: WidgetMetricOption
    let snapshot: WidgetDisplaySnapshot
    let isChinese: Bool

    var body: some View {
        Group {
            if metric.isQuota {
                QuotaCardView(metric: metric, snapshot: snapshot, isChinese: isChinese)
            } else {
                BalanceCardView(metric: metric, snapshot: snapshot, isChinese: isChinese)
            }
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}

struct QuotaCardView: View {
    let metric: WidgetMetricOption
    let snapshot: WidgetDisplaySnapshot
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
                .font(.system(size: size, weight: .semibold, design: .rounded).monospacedDigit())
            if value != nil { Text("%").font(.system(size: size / 2, weight: .medium)) }
        }
        .foregroundStyle(.primary)
        .lineLimit(1)
    }

    private func quotaColor(_ value: Double?) -> Color {
        guard let value else { return .secondary }
        return value > 70 ? .blue : (value >= 30 ? .orange : .red)
    }

    var body: some View {
        if !isGoogle, let accounts = snapshot.resolvedCodexAccounts, accounts.count > 1 {
            poolOverviewBody(accounts: accounts)
        } else {
            compactBody
        }
    }

    private var compactBody: some View {
        let data = quotaData
        return VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 3) {
                Text(data.title).font(.system(size: 10, weight: .semibold)).foregroundStyle(.secondary)
                    .lineLimit(1).minimumScaleFactor(0.85)
                Spacer(minLength: 0)
                if data.state == "stale" || data.state == "reauth" {
                    Text(data.state == "reauth" ? (isChinese ? "登录" : "Sign in") : (isChinese ? "缓存" : "Cached"))
                        .font(.system(size: 8)).foregroundStyle(.orange)
                }
            }
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                if !data.isWeekly && data.primary != nil {
                    Text("5h").font(.system(size: 9)).foregroundStyle(.secondary)
                }
                quotaNumber(data.primary, size: 20)
                    .accessibilityLabel(data.isWeekly ? (isChinese ? "周剩余额度" : "Weekly remaining") : (isChinese ? "五小时剩余额度" : "Five-hour remaining"))
                    .accessibilityValue(data.primary.map { String(format: "%.0f%%", $0) } ?? "—")
                Spacer(minLength: 0)
                if let secondary = data.secondary, secondary < 100 {
                    Text("5h " + String(format: "%.0f%%", secondary))
                        .font(.system(size: 8.5).monospacedDigit()).foregroundStyle(quotaColor(secondary))
                        .fixedSize()
                } else if isGoogle && (!data.isWeekly || data.primary == nil) {
                    Text(data.primary == nil ? (isChinese ? "暂无数据" : "No data") : (isChinese ? "无周额度" : "No weekly"))
                        .font(.system(size: 8)).foregroundStyle(.secondary)
                        .lineLimit(1).minimumScaleFactor(0.7)
                }
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.primary.opacity(0.06))
                    if let primary = data.primary {
                        Capsule().fill(quotaColor(primary)).frame(width: geo.size.width * min(max(primary, 0), 100) / 100)
                    }
                }
            }
            .frame(height: 3)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private func poolOverviewBody(accounts: [WidgetCodexAccount]) -> some View {
        let isStale = snapshot.stale || snapshot.codexStale
        let topAccounts = Array(accounts.prefix(2))
        let extraCount = accounts.count - topAccounts.count
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(snapshot.isCodexAutoPool ? (isChinese ? "Codex · 自动" : "Codex · Auto") : "Codex")
                    .font(.system(size: 10, weight: .semibold))
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
                let primary = week ?? five
                let isActive = snapshot.codexSelectionMode != "auto" &&
                    (snapshot.codexActiveAccountID.map { $0 == acct.id } ?? (acct.active == true))
                VStack(alignment: .leading, spacing: 1) {
                    HStack(alignment: .firstTextBaseline, spacing: 3) {
                        Text(acct.safeDisplayName)
                            .font(.system(size: 9, weight: isActive ? .semibold : .regular))
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                            .accessibilityLabel(acct.safeDisplayName + (isActive ? (isChinese ? "，当前账号" : ", Active") : ""))
                        Spacer(minLength: 0)
                        if acct.needsReauth == true {
                            Text(isChinese ? "登录" : "Sign in").font(.system(size: 8.5)).foregroundStyle(.orange)
                        } else {
                            if acct.stale == true && !isStale {
                                Text(isChinese ? "缓存" : "Cached").font(.system(size: 8)).foregroundStyle(.orange)
                            }
                            HStack(spacing: 4) {
                                Text((week == nil && five != nil ? "5h " : "") + (primary.map { String(format: "%.0f%%", $0) } ?? "—"))
                                    .foregroundStyle(.primary)
                                    .accessibilityLabel(week != nil ? (isChinese ? "周剩余额度" : "Weekly remaining") : (isChinese ? "五小时剩余额度" : "Five-hour remaining"))
                                    .accessibilityValue(primary.map { String(format: "%.0f%%", $0) } ?? "—")
                                if week != nil, let five, five < 100 {
                                    Text("5h " + String(format: "%.0f%%", five))
                                        .foregroundStyle(quotaColor(five))
                                }
                            }
                            .font(.system(size: 9, weight: .medium).monospacedDigit())
                            .fixedSize()
                        }
                    }
                    GeometryReader { geo in
                        ZStack(alignment: .leading) {
                            Capsule().fill(Color.primary.opacity(0.06))
                            if let w = primary {
                                Capsule().fill(quotaColor(w)).frame(width: geo.size.width * min(max(w, 0), 100) / 100)
                            }
                        }
                    }
                    .frame(height: 2)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .combine)
    }
}

struct BalanceCardView: View {
    let metric: WidgetMetricOption
    let snapshot: WidgetDisplaySnapshot
    let isChinese: Bool

    private var balanceData: (title: String, valueText: String, unit: String) {
        if metric == .workbuddy {
            return (
                "WorkBuddy",
                snapshot.workbuddyPointsText,
                isChinese ? "积分" : "pts"
            )
        } else {
            return (
                "DeepSeek",
                snapshot.deepseekBalanceText,
                snapshot.deepseekCurrency
            )
        }
    }

    var body: some View {
        compactBody
    }

    private var compactBody: some View {
        let data = balanceData
        let status = metric == .workbuddy ? snapshot.workbuddyCompactStatusText : snapshot.deepseekCompactStatusText
        let state = metric == .workbuddy ? snapshot.workbuddyState : snapshot.deepseekState
        return VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Text(data.title).font(.system(size: 10, weight: .semibold)).foregroundStyle(.secondary)
                    .lineLimit(1).minimumScaleFactor(0.85)
                Spacer(minLength: 0)
                if !status.isEmpty {
                    Text(state == "stale" ? (isChinese ? "缓存" : "Cached") :
                        (state == "unavailable" ? (isChinese ? "不可用" : "Unavailable") : (isChinese ? "检查账户" : "Check account")))
                        .font(.system(size: 8)).foregroundStyle(.orange).lineLimit(1)
                }
            }
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(data.valueText)
                    .font(.system(size: 20, weight: .semibold, design: .rounded).monospacedDigit())
                    .lineLimit(1).minimumScaleFactor(0.7)
                Text(data.unit).font(.system(size: 9)).foregroundStyle(.secondary).fixedSize()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .combine)
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
        .overlay(alignment: .bottomTrailing) {
            refreshButton
                .padding(.trailing, 18)
                .padding(.bottom, 12)
        }
        .containerBackground(.fill.tertiary, for: .widget)
    }

    private var refreshButton: some View {
        Button(intent: RefreshWidgetIntent()) {
            Image(systemName: "arrow.clockwise")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 18, height: 14)
        }
        .buttonStyle(.plain)
        .contentShape(Rectangle())
        .accessibilityLabel(isChinese ? "刷新小组件" : "Refresh Widget")
    }

    private var mediumContent: some View {
        let metrics = entry.configuration.resolvedMetrics(for: .systemMedium)
        return GeometryReader { geometry in
            let width = (geometry.size.width - 6) / 2
            let height = (geometry.size.height - 6) / 2
            VStack(spacing: 6) {
                ForEach(0..<2) { row in
                    HStack(spacing: 6) {
                        ForEach(0..<2) { column in
                            MetricCardView(metric: metrics[row * 2 + column], snapshot: entry.snapshot, isChinese: isChinese)
                                .frame(width: width, height: height)
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    private var smallContent: some View {
        let metrics = entry.configuration.resolvedMetrics(for: .systemSmall)
        return GeometryReader { geometry in
            VStack(spacing: 6) {
                ForEach(metrics, id: \.self) { metric in
                    MetricCardView(metric: metric, snapshot: entry.snapshot, isChinese: isChinese)
                        .frame(height: (geometry.size.height - 6) / 2)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }
}
