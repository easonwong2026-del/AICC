import SwiftUI
import WidgetKit

struct AICCWidgetEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetDisplaySnapshot
}

struct AICCWidgetProvider: TimelineProvider {
    func placeholder(in context: Context) -> AICCWidgetEntry {
        AICCWidgetEntry(date: .now, snapshot: .placeholder)
    }

    func getSnapshot(in context: Context, completion: @escaping (AICCWidgetEntry) -> Void) {
        if context.isPreview {
            completion(AICCWidgetEntry(date: .now, snapshot: .placeholder))
            return
        }

        Task {
            let snapshot = await WidgetStatusLoader.snapshot()
            completion(AICCWidgetEntry(date: .now, snapshot: snapshot))
        }
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<AICCWidgetEntry>) -> Void) {
        Task {
            let now = Date.now
            let snapshot = await WidgetStatusLoader.snapshot()
            let entry = AICCWidgetEntry(date: now, snapshot: snapshot)
            var dates = [snapshot.fetchedAt.addingTimeInterval(WidgetDisplaySnapshot.liveLifetime),
                         snapshot.fetchedAt.addingTimeInterval(WidgetDisplaySnapshot.cacheLifetime)]
            if let epoch = snapshot.google?.updated_epoch {
                let source = Date(timeIntervalSince1970: epoch)
                dates += [source.addingTimeInterval(WidgetDisplaySnapshot.liveLifetime),
                          source.addingTimeInterval(WidgetDisplaySnapshot.cacheLifetime)]
            }
            let future = Array(Set(dates.filter { $0 > now })).sorted()
            let entries = [entry] + future.map { date in
                AICCWidgetEntry(date: date, snapshot: snapshot.evaluated(at: date))
            }
            completion(Timeline(entries: entries, policy: .after(now.addingTimeInterval(5 * 60))))
        }
    }
}

struct AICCWidgetView: View {
    @Environment(\.locale) private var locale
    @Environment(\.widgetFamily) private var family
    var familyOverride: WidgetFamily? = nil

    let entry: AICCWidgetEntry

    var body: some View {
        Group {
            if (familyOverride ?? family) == .systemMedium {
                mediumContent
            } else {
                smallContent
            }
        }
        .containerBackground(.fill.tertiary, for: .widget)
    }

    private func label(_ chinese: String, _ english: String) -> String {
        locale.identifier.hasPrefix("zh") ? chinese : english
    }

    private var refreshButton: some View {
        Button(intent: RefreshWidgetIntent()) {
            Image(systemName: "arrow.clockwise")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Refresh Widget")
    }

    private var mediumContent: some View {
        VStack(spacing: 6) {
            HStack {
                Text("AICC").font(.system(size: 11, weight: .bold))
                Spacer()
                refreshButton
            }
            HStack(alignment: .top, spacing: 16) {
                quotaView(google: false, compact: false)
                quotaView(google: true, compact: false)
            }
            Spacer(minLength: 0)
            Divider()
            balances(compact: false)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private var smallContent: some View {
        VStack(spacing: 5) {
            HStack {
                Text("AICC").font(.system(size: 11, weight: .bold))
                Spacer()
                refreshButton
            }
            quotaView(google: false, compact: true)
            quotaView(google: true, compact: true)
            Spacer(minLength: 0)
            balances(compact: true)
        }
        .padding(10)
    }

    private func quotaView(google: Bool, compact: Bool) -> some View {
        let snapshot = entry.snapshot
        let quota = snapshot.google
        let primary = google ? quota?.primary?.remaining : (snapshot.codexWeeklyRemaining ?? snapshot.codexFiveHourRemaining)
        let secondary = google ? quota?.secondary?.remaining : snapshot.codexSecondaryFiveHourRemaining
        let weekly = google ? quota?.isWeekly == true : snapshot.codexWeeklyRemaining != nil
        let state = google ? snapshot.googleState : snapshot.codexState
        let title = (google ? "Google" : "Codex") + (primary == nil ? "" : (weekly ? label(" 周", " Weekly") : " 5h"))
        let reset = google ? quota?.reset.map { String($0.dropFirst($0.count >= 16 ? 5 : 0)) } : snapshot.codexResetShortText?.replacingOccurrences(of: "重置于 ", with: "")
        return VStack(alignment: .leading, spacing: compact ? 2 : 3) {
            HStack(alignment: .lastTextBaseline, spacing: 3) {
                Text(title).font(.system(size: compact ? 10 : 11, weight: .medium)).foregroundStyle(.secondary)
                if state == "stale" && !compact {
                    Text(label("缓存", "Cached")).font(.system(size: 8)).foregroundStyle(.orange)
                }
                if compact {
                    Spacer(minLength: 0)
                    quotaNumber(primary, size: 19)
                }
            }
            .lineLimit(1)
            if !compact { quotaNumber(primary, size: 32) }
            if let primary {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.primary.opacity(0.1))
                        Capsule().fill(quotaColor(primary)).frame(width: geo.size.width * min(max(primary, 0), 100) / 100)
                    }
                }
                .frame(height: compact ? 3 : 5)
            }
            HStack(spacing: 2) {
                if !compact, let reset {
                    Text(reset).foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                }
                if let secondary {
                    Text("5h").foregroundStyle(.secondary)
                    Text(String(format: "%.0f%%", secondary)).foregroundStyle(quotaColor(secondary))
                } else if google && (!weekly || primary == nil) {
                    Text(primary == nil ? label("暂无数据", "No data") : label("周额度暂无数据", "Weekly unavailable"))
                        .foregroundStyle(.secondary)
                }
                if compact && state == "stale" {
                    Spacer(minLength: 0)
                    Text(label("缓存", "Cached")).foregroundStyle(.orange)
                }
            }
            .font(.system(size: compact ? 9 : 9.5))
            .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
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

    private func balances(compact: Bool) -> some View {
        let snapshot = entry.snapshot
        return HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 1) {
                Text("WorkBuddy" + (snapshot.workbuddyState == "stale" ? label(" · 缓存", " · Cached") : ""))
                    .foregroundStyle(.secondary)
                Text(snapshot.workbuddyPointsText + (compact ? "" : label(" 积分", " pts")))
                    .fontWeight(.semibold)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(alignment: .leading, spacing: 1) {
                Text("DeepSeek" + (snapshot.deepseekState == "stale" ? label(" · 缓存", " · Cached") : ""))
                    .foregroundStyle(.secondary)
                Text(snapshot.deepseekBalanceText + " " + snapshot.deepseekCurrency)
                    .fontWeight(.semibold)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(.system(size: compact ? 8.5 : 11))
        .lineLimit(1)
        .minimumScaleFactor(0.85)
    }
}

@main
struct AICCWidget: Widget {
    nonisolated static let kind = "com.aieink.dashboard.menubar.widget"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: Self.kind, provider: AICCWidgetProvider()) { entry in
            AICCWidgetView(entry: entry)
        }
        .configurationDisplayName("AICC")
        .description("AICC status at a glance")
        .supportedFamilies([.systemSmall, .systemMedium])
        .contentMarginsDisabled()
    }
}
