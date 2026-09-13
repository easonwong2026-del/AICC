import SwiftUI
import WidgetKit
import AppIntents

struct AICCWidgetProvider: AppIntentTimelineProvider {
    typealias Entry = AICCWidgetEntry
    typealias Intent = AICCWidgetConfigurationIntent

    func placeholder(in context: Context) -> AICCWidgetEntry {
        AICCWidgetEntry(date: .now, snapshot: .placeholder, configuration: AICCWidgetConfigurationIntent())
    }

    func snapshot(for configuration: AICCWidgetConfigurationIntent, in context: Context) async -> AICCWidgetEntry {
        if context.isPreview {
            return AICCWidgetEntry(date: .now, snapshot: .placeholder, configuration: configuration)
        }

        let snapshot = await WidgetStatusLoader.snapshot()
        return AICCWidgetEntry(date: .now, snapshot: snapshot, configuration: configuration)
    }

    func timeline(for configuration: AICCWidgetConfigurationIntent, in context: Context) async -> Timeline<AICCWidgetEntry> {
        let now = Date.now
        let snapshot = await WidgetStatusLoader.snapshot()
        let entry = AICCWidgetEntry(date: now, snapshot: snapshot, configuration: configuration)
        var dates = [snapshot.fetchedAt.addingTimeInterval(WidgetDisplaySnapshot.liveLifetime),
                     snapshot.fetchedAt.addingTimeInterval(WidgetDisplaySnapshot.cacheLifetime)]
        if let epoch = snapshot.google?.updated_epoch {
            let source = Date(timeIntervalSince1970: epoch)
            dates += [source.addingTimeInterval(WidgetDisplaySnapshot.liveLifetime),
                      source.addingTimeInterval(WidgetDisplaySnapshot.cacheLifetime)]
        }
        let future = Array(Set(dates.filter { $0 > now })).sorted()
        let entries = [entry] + future.map { date in
            AICCWidgetEntry(date: date, snapshot: snapshot.evaluated(at: date), configuration: configuration)
        }
        return Timeline(entries: entries, policy: .after(now.addingTimeInterval(5 * 60)))
    }
}

@main
struct AICCWidget: Widget {
    nonisolated static let kind = "com.aieink.dashboard.menubar.widget"

    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: Self.kind, intent: AICCWidgetConfigurationIntent.self, provider: AICCWidgetProvider()) { entry in
            AICCWidgetView(entry: entry)
        }
        .configurationDisplayName("AICC")
        .description("AICC status at a glance")
        .supportedFamilies([.systemSmall, .systemMedium])
        .contentMarginsDisabled()
    }
}
