import SwiftUI
import WidgetKit
import AppIntents

struct AICCWidgetProvider: AppIntentTimelineProvider {
    typealias Entry = AICCWidgetEntry
    typealias Intent = AICCWidgetConfigurationIntent

    func placeholder(in context: Context) -> AICCWidgetEntry {
        AICCWidgetEntry(date: .now, snapshot: .magpiePlaceholder, configuration: AICCWidgetConfigurationIntent())
    }

    func snapshot(for configuration: AICCWidgetConfigurationIntent, in context: Context) async -> AICCWidgetEntry {
        if context.isPreview {
            return AICCWidgetEntry(date: .now, snapshot: .magpiePlaceholder, configuration: configuration)
        }

        var snapshot = await WidgetStatusLoader.snapshot(port: configuration.magpiePort)
        snapshot.codexAccountDisplayMode = configuration.accounts.mode
        return AICCWidgetEntry(date: .now, snapshot: snapshot, configuration: configuration)
    }

    func timeline(for configuration: AICCWidgetConfigurationIntent, in context: Context) async -> Timeline<AICCWidgetEntry> {
        let now = Date.now
        var snapshot = await WidgetStatusLoader.snapshot(port: configuration.magpiePort)
        snapshot.codexAccountDisplayMode = configuration.accounts.mode
        let entry = AICCWidgetEntry(date: now, snapshot: snapshot, configuration: configuration)
        var dates = [snapshot.fetchedAt.addingTimeInterval(WidgetDisplaySnapshot.liveLifetime),
                     snapshot.fetchedAt.addingTimeInterval(WidgetDisplaySnapshot.cacheLifetime)]
        if let epoch = snapshot.google?.updated_epoch {
            let source = Date(timeIntervalSince1970: epoch)
            dates += [source.addingTimeInterval(WidgetDisplaySnapshot.liveLifetime),
                      source.addingTimeInterval(WidgetDisplaySnapshot.cacheLifetime)]
        }
        dates += (snapshot.codexAccounts ?? []).compactMap { $0.readAt }.map {
            Date(timeIntervalSince1970: $0).addingTimeInterval(300)
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
        .description("Codex quotas from Magpie. Works while AICC is closed.")
        .supportedFamilies([.systemSmall, .systemMedium])
        .contentMarginsDisabled()
    }
}
