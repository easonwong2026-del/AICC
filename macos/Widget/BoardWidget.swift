import SwiftUI
import WidgetKit

/// Four pages, one per reading, cycled by the timeline rather than by a gesture:
/// a desktop widget has no swipe, and WidgetKit's schedule is the only clock it
/// can trust.  The footer buttons hand the wheel to the reader for a few
/// minutes; the loop then picks itself up again.
struct BoardWidgetProvider: TimelineProvider {
    typealias Entry = BoardEntry

    /// Seconds a page stays up.  Four pages make a 60 second loop.
    static let pageSeconds: TimeInterval = 15
    /// How long one tap holds the carousel before it resumes on its own.
    static let manualSeconds: TimeInterval = 300
    /// A held page still gets re-read, just this slowly.
    static let heldRefreshSeconds: TimeInterval = 60
    /// Loops per timeline before WidgetKit is asked to read Magpie again.
    static let rounds = 2

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
        let port = configuration.magpiePort
        let now = Date.now
        let pages = BoardPage.allCases

        // A page turn asked for from the host app (the footer links open it)
        // lands here: apply it once, then the hold below keeps the page still.
        if let command = WidgetPreferences.boardCommand(),
           BoardCursor.consume(command, port: port, now: now) {
            return await timeline()
        }

        let snapshot = await WidgetUsageLoader.load(port: port)

        // A tapped page stands still, with a slow refresh of its own so the
        // numbers do not go stale while the page is being read.
        if let held = BoardCursor.hold(port: port, now: now) {
            var entries: [BoardEntry] = []
            var date = now
            repeat {
                entries.append(BoardEntry(date: date, snapshot: snapshot.evaluated(at: date),
                                          page: held.page, port: port, manual: true))
                date = date.addingTimeInterval(Self.heldRefreshSeconds)
            } while date < held.until
            return Timeline(entries: entries, policy: .after(held.until))
        }

        // Resume on the page the last hold ended on, so the loop never jumps back.
        let start = BoardCursor.resumePage(port: port)
        BoardCursor.saveAnchor(BoardCursor.Anchor(page: start, date: now), port: port)

        var entries: [BoardEntry] = []
        // Two loops of pages, then ask for a reload so the numbers are re-read.
        for round in 0..<Self.rounds {
            for index in 0..<pages.count {
                let offset = Double(round * pages.count + index) * Self.pageSeconds
                let date = now.addingTimeInterval(offset)
                let page = pages[(start.rawValue + round * pages.count + index) % pages.count]
                entries.append(BoardEntry(date: date, snapshot: snapshot.evaluated(at: date),
                                          page: page, port: port))
            }
        }
        return Timeline(entries: entries,
                        policy: .after(now.addingTimeInterval(Self.pageSeconds * Double(pages.count * Self.rounds))))
    }
}

/// Where the board was left.  The provider writes the loop's anchor and the
/// buttons write the hold, both in the widget's own defaults, so a tap moves on
/// from the page that is on screen instead of from a guess.
enum BoardCursor {
    struct Anchor: Equatable {
        var page: BoardPage
        var date: Date
    }

    struct Hold: Equatable {
        var page: BoardPage
        var until: Date
    }

    private static func anchorPageKey(_ port: Int) -> String { "aicc.board.anchor.page.\(port)" }
    private static func anchorDateKey(_ port: Int) -> String { "aicc.board.anchor.date.\(port)" }
    private static func holdPageKey(_ port: Int) -> String { "aicc.board.hold.page.\(port)" }
    private static func holdUntilKey(_ port: Int) -> String { "aicc.board.hold.until.\(port)" }
    private static let commandAppliedKey = "aicc.board.command.applied"

    static func anchor(port: Int, defaults: UserDefaults = .standard) -> Anchor? {
        guard let raw = defaults.object(forKey: anchorPageKey(port)) as? Int,
              let page = BoardPage(rawValue: raw) else { return nil }
        let stamp = defaults.double(forKey: anchorDateKey(port))
        guard stamp > 0 else { return nil }
        return Anchor(page: page, date: Date(timeIntervalSince1970: stamp))
    }

    static func saveAnchor(_ anchor: Anchor, port: Int, defaults: UserDefaults = .standard) {
        defaults.set(anchor.page.rawValue, forKey: anchorPageKey(port))
        defaults.set(anchor.date.timeIntervalSince1970, forKey: anchorDateKey(port))
    }

    static func hold(port: Int, now: Date = .now, defaults: UserDefaults = .standard) -> Hold? {
        guard let raw = defaults.object(forKey: holdPageKey(port)) as? Int,
              let page = BoardPage(rawValue: raw) else { return nil }
        let until = Date(timeIntervalSince1970: defaults.double(forKey: holdUntilKey(port)))
        guard until > now else { return nil }
        return Hold(page: page, until: until)
    }

    static func setHold(_ hold: Hold, port: Int, defaults: UserDefaults = .standard) {
        defaults.set(hold.page.rawValue, forKey: holdPageKey(port))
        defaults.set(hold.until.timeIntervalSince1970, forKey: holdUntilKey(port))
    }

    /// Apply a page turn requested by the host app, once per request.
    ///
    /// The request carries the stamp it was made at; the extension records the
    /// last stamp it honoured, so a second timeline build for the same tap is a
    /// no-op.  Returns true when this call did the work, and the caller rebuilds
    /// its timeline around the new hold.
    @discardableResult
    static func consume(_ command: WidgetPreferences.BoardCommand, port: Int, now: Date = .now,
                        defaults: UserDefaults = .standard) -> Bool {
        let applied = defaults.double(forKey: commandAppliedKey)
        guard command.at > applied else { return false }
        defaults.set(command.at, forKey: commandAppliedKey)
        _ = step(command.step, port: port, now: now, defaults: defaults)
        return true
    }

    /// The page the tile is showing right now, derived from the anchor its
    /// timeline was built on: WidgetKit draws entries by date, so this matches
    /// the screen unless a reload arrived late.
    static func visiblePage(port: Int, now: Date = .now,
                            pageSeconds: TimeInterval = BoardWidgetProvider.pageSeconds,
                            defaults: UserDefaults = .standard) -> BoardPage {
        if let held = hold(port: port, now: now, defaults: defaults) { return held.page }
        guard let anchor = anchor(port: port, defaults: defaults) else { return .overview }
        let elapsed = max(0, now.timeIntervalSince(anchor.date))
        let step = Int(elapsed / max(1, pageSeconds))
        let count = BoardPage.allCases.count
        return BoardPage(rawValue: (anchor.page.rawValue + step) % count) ?? .overview
    }

    /// Continue from the page the last hold ended on, so the loop never jumps back.
    static func resumePage(port: Int, defaults: UserDefaults = .standard) -> BoardPage {
        guard let raw = defaults.object(forKey: holdPageKey(port)) as? Int,
              let page = BoardPage(rawValue: raw) else { return .overview }
        return page
    }

    @discardableResult
    static func step(_ delta: Int, port: Int, now: Date = .now,
                     defaults: UserDefaults = .standard) -> BoardPage {
        let count = BoardPage.allCases.count
        let current = visiblePage(port: port, now: now, defaults: defaults)
        let target = ((current.rawValue + delta) % count + count) % count
        let page = BoardPage(rawValue: target) ?? .overview
        setHold(Hold(page: page, until: now.addingTimeInterval(BoardWidgetProvider.manualSeconds)),
                port: port, defaults: defaults)
        return page
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
        .description("Magpie 的调用、延迟、费用与缓存命中，四页自动轮播，也可手动翻页。")
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
