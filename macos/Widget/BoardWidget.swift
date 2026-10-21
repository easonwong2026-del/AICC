import SwiftUI
import WidgetKit

/// Four pages, one per reading, cycled by the timeline rather than by a gesture:
/// a desktop widget has no swipe, and WidgetKit's schedule is the only clock it
/// can trust.
struct BoardWidgetProvider: TimelineProvider {
    typealias Entry = BoardEntry

    /// Seconds a page stays up.  Four pages make a 60 second loop.
    static let pageSeconds: TimeInterval = 15

    func placeholder(in context: Context) -> BoardEntry {
        BoardEntry(date: .now, snapshot: BoardFixture.snapshot, page: .overview, port: 3425)
    }

    func getSnapshot(in context: Context, completion: @escaping (BoardEntry) -> Void) {
        let configuration = AICCWidgetConfiguration.load()
        if context.isPreview {
            completion(BoardEntry(date: .now, snapshot: BoardFixture.snapshot,
                                  page: .overview, port: configuration.magpiePort))
            return
        }
        Task {
            let snapshot = await WidgetUsageLoader.load(port: configuration.magpiePort)
            completion(BoardEntry(date: .now, snapshot: snapshot, page: .overview,
                                  port: configuration.magpiePort))
        }
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<BoardEntry>) -> Void) {
        Task { completion(await timeline()) }
    }

    private func timeline() async -> Timeline<BoardEntry> {
        let configuration = AICCWidgetConfiguration.load()
        let snapshot = await WidgetUsageLoader.load(port: configuration.magpiePort)
        let now = Date.now
        let pages = BoardPage.allCases
        var entries: [BoardEntry] = []
        // Two loops of pages, then ask for a reload so the numbers are re-read.
        for round in 0..<2 {
            for (index, page) in pages.enumerated() {
                let offset = Double(round * pages.count + index) * Self.pageSeconds
                let date = now.addingTimeInterval(offset)
                entries.append(BoardEntry(date: date, snapshot: snapshot.evaluated(at: date),
                                          page: page, port: configuration.magpiePort))
            }
        }
        return Timeline(entries: entries,
                        policy: .after(now.addingTimeInterval(Self.pageSeconds * Double(pages.count * 2))))
    }
}

struct AICCBoardWidget: Widget {
    nonisolated static let kind = AICCWidgetMetadata.boardKind

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: Self.kind, provider: BoardWidgetProvider()) { entry in
            BoardView(entry: entry)
                .widgetURL(AICCWidgetMetadata.refreshURL)
        }
        .configurationDisplayName("AICC 看板")
        .description("Magpie 的调用、延迟、费用与缓存命中，四页自动轮播。")
        .supportedFamilies([.systemLarge])
        .contentMarginsDisabled()
    }
}

/// Preview and placeholder numbers, close to a real weekday on this machine.
enum BoardFixture {
    static var snapshot: UsageSnapshot {
        var snapshot = UsageSnapshot()
        snapshot.generatedAt = Date.now.timeIntervalSince1970
        snapshot.available = true
        snapshot.remainingPercent = 64
        snapshot.remainingWindow = "Codex · 7 天"
        snapshot.resetSeconds = 5 * 86400 + 22 * 3600
        snapshot.callsToday = 965
        snapshot.errorsToday = 15
        snapshot.successPermille = 985
        snapshot.tokensToday = 1_240_000
        snapshot.tokens7d = 8_600_000
        snapshot.averageTTFTMillis = 1_800
        snapshot.latestModel = "deepseek-v4.1-flash"
        snapshot.latestStatus = 200
        snapshot.latestLatencyMillis = 12_400
        snapshot.recent = [RecentRequest(model: "deepseek-v4.1-flash", status: 200, millis: 12_400),
                           RecentRequest(model: "gpt-5.6-luna", status: 200, millis: 7_100),
                           RecentRequest(model: "claude-sonnet-5", status: 429, millis: 900)]
        snapshot.sessionCount = 93
        snapshot.totalCalls = 6_509
        snapshot.totalInput = 59_016_319
        snapshot.totalOutput = 2_521_394
        snapshot.totalCacheRead = 944_403_004
        snapshot.totalReasoning = 809_870
        snapshot.costMicros = 156_648_129
        snapshot.cacheHitPermille = 941
        snapshot.topModel = "gpt-5.6-luna"
        snapshot.topModelCalls = 905
        return snapshot
    }
}
