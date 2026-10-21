import SwiftUI
import WidgetKit

struct AICCWidgetProvider: TimelineProvider {
    typealias Entry = AICCWidgetEntry

    func placeholder(in context: Context) -> AICCWidgetEntry {
        AICCWidgetEntry(date: .now, snapshot: .placeholder(), configuration: AICCWidgetConfiguration())
    }

    func getSnapshot(in context: Context, completion: @escaping (AICCWidgetEntry) -> Void) {
        let configuration = AICCWidgetConfiguration.load()
        if context.isPreview {
            completion(AICCWidgetEntry(date: .now, snapshot: .placeholder(), configuration: configuration))
            return
        }
        Task {
            let snapshot = await WidgetStatusLoader.snapshot(port: configuration.magpiePort)
            completion(AICCWidgetEntry(date: .now, snapshot: snapshot, configuration: configuration))
        }
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<AICCWidgetEntry>) -> Void) {
        Task { completion(await timeline()) }
    }

    private func timeline() async -> Timeline<AICCWidgetEntry> {
        let configuration = AICCWidgetConfiguration.load()
        let now = Date.now
        let snapshot = await WidgetStatusLoader.snapshot(port: configuration.magpiePort)
        let entry = AICCWidgetEntry(date: now, snapshot: snapshot, configuration: configuration)
        // WidgetKit does not honour an exact timer: this only asks for a reload
        // around the moment the reading stops being fresh.
        var dates: [Date] = [Date(timeIntervalSince1970: snapshot.fetchedAt)
            .addingTimeInterval(QuotaSnapshot.liveLifetime)]
        dates += snapshot.accounts.compactMap { $0.readAt }.map {
            Date(timeIntervalSince1970: $0).addingTimeInterval(QuotaSnapshot.liveLifetime)
        }
        let future = Array(Set(dates.filter { $0 > now })).sorted()
        let entries = [entry] + future.map { date in
            AICCWidgetEntry(date: date, snapshot: snapshot.evaluated(at: date), configuration: configuration)
        }
        return Timeline(entries: entries, policy: .after(now.addingTimeInterval(QuotaPolicy.refreshSeconds)))
    }
}

struct AICCWidget: Widget {
    nonisolated static let kind = AICCWidgetMetadata.kind

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: Self.kind, provider: AICCWidgetProvider()) { entry in
            AICCWidgetView(entry: entry)
                .widgetURL(AICCWidgetMetadata.refreshURL)
        }
        .configurationDisplayName("AICC")
        .description("Magpie 中所有服务商的额度。关闭 AICC 也能更新。")
        .supportedFamilies([.systemSmall, .systemMedium])
        .contentMarginsDisabled()
    }
}

@main
struct AICCWidgetBundle: WidgetBundle {
    var body: some Widget {
        AICCWidget()
        AICCBoardWidget()
    }
}
