import SwiftUI
import WidgetKit
import AppIntents

enum BoardPage: Int, CaseIterable, Codable, Sendable {
    case overview, requests, sessions, context

    var title: String {
        switch self {
        case .overview: return "总览"
        case .requests: return "请求"
        case .sessions: return "会话"
        case .context: return "上下文"
        }
    }

    var range: String {
        switch self {
        case .overview, .requests: return "今日"
        case .sessions, .context: return "30 天"
        }
    }
}

/// The device's palette from `src/ui.cpp` (MIT), on a desktop background.
struct BoardPalette {
    let background: Color
    let card: Color
    let track: Color
    let divider: Color
    let ink: Color
    let muted: Color
    let accent: Color
    let amber: Color
    let red: Color
    let cyan: Color
    /// A hairline around cards: a white card on a white desktop page would
    /// otherwise have no edge at all.
    let edge: Color
    var accented: Bool = false
    var isDark: Bool = true

    static func of(_ scheme: ColorScheme, mode: WidgetRenderingMode = .fullColor) -> BoardPalette {
        if mode != .fullColor {
            // macOS 26 dims desktop widgets: every colour is tinted white and
            // the background is dropped, so only opacity survives.
            return BoardPalette(background: .clear,
                                card: Color(.sRGB, white: 1, opacity: 0.13),
                                track: Color(.sRGB, white: 1, opacity: 0.16),
                                divider: Color(.sRGB, white: 1, opacity: 0.22),
                                ink: Color(.sRGB, white: 1, opacity: 1),
                                muted: Color(.sRGB, white: 1, opacity: 0.66),
                                accent: Color(.sRGB, white: 1, opacity: 1),
                                amber: Color(.sRGB, white: 1, opacity: 0.9),
                                red: Color(.sRGB, white: 1, opacity: 0.9),
                                cyan: Color(.sRGB, white: 1, opacity: 0.86),
                                edge: Color(.sRGB, white: 1, opacity: 0.10),
                                accented: true,
                                isDark: true)
        }
        return scheme == .dark
            ? BoardPalette(background: Color(red: 0x0B / 255, green: 0x12 / 255, blue: 0x10 / 255),
                           card: Color(red: 0x12 / 255, green: 0x1A / 255, blue: 0x16 / 255),
                           track: Color(red: 0x1A / 255, green: 0x23 / 255, blue: 0x1E / 255),
                           divider: Color(red: 0x1E / 255, green: 0x2A / 255, blue: 0x25 / 255),
                           ink: Color(red: 0xF4 / 255, green: 0xF7 / 255, blue: 0xED / 255),
                           muted: Color(red: 0x86 / 255, green: 0x95 / 255, blue: 0x8D / 255),
                           accent: Color(red: 0xA8 / 255, green: 0xFF / 255, blue: 0x4F / 255),
                           amber: Color(red: 0xFF / 255, green: 0xB0 / 255, blue: 0x2E / 255),
                           red: Color(red: 0xFF / 255, green: 0x5F / 255, blue: 0x57 / 255),
                           cyan: Color(red: 0x55 / 255, green: 0xE6 / 255, blue: 0xD2 / 255),
                           edge: Color(.sRGB, white: 1, opacity: 0.05),
                           isDark: true)
            : BoardPalette(background: Color(red: 0xF7 / 255, green: 0xF8 / 255, blue: 0xF3 / 255),
                           card: .white,
                           track: Color(red: 0xE4 / 255, green: 0xE6 / 255, blue: 0xDE / 255),
                           divider: Color(red: 0xD9 / 255, green: 0xDC / 255, blue: 0xD2 / 255),
                           ink: Color(red: 0x14 / 255, green: 0x17 / 255, blue: 0x0F / 255),
                           muted: Color(red: 0x63 / 255, green: 0x68 / 255, blue: 0x5C / 255),
                           accent: Color(red: 0x4E / 255, green: 0x8C / 255, blue: 0x0E / 255),
                           amber: Color(red: 0xA9 / 255, green: 0x69 / 255, blue: 0x0A / 255),
                           red: Color(red: 0xC0 / 255, green: 0x39 / 255, blue: 0x2B / 255),
                           cyan: Color(red: 0x0E / 255, green: 0x8C / 255, blue: 0x7C / 255),
                           edge: Color(red: 0xE1 / 255, green: 0xE5 / 255, blue: 0xDA / 255),
                           isDark: false)
    }
}

/// Type is set in real points, not scaled from the 480 unit design grid: the
/// tile is a fixed 338×354 and the old `× s` sizes landed on the 9pt floor,
/// which is where "the labels look too small" came from.  Spacing still follows
/// `s` so the proportions of the panel are kept.
private enum BoardText {
    /// Brand line and status.
    static let brand: CGFloat = 11.5
    static let status: CGFloat = 11
    /// Page title, and the muted range beside it.
    static let pageTitle: CGFloat = 18
    static let range: CGFloat = 10.5
    /// Section labels: 最近模型, 会话, 缓存命中.
    static let section: CGFloat = 11
    /// Card label and its number.
    static let cardLabel: CGFloat = 11
    static let cardValue: CGFloat = 17.5
    /// A recent request line and the muted summary under the pages.
    static let row: CGFloat = 11.5
    static let summary: CGFloat = 10.5
    /// Footer count.
    static let footer: CGFloat = 10.5
    /// The big numbers.
    static let hero: CGFloat = 40
    static let heroFlat: CGFloat = 34
    static let model: CGFloat = 16
}

struct BoardEntry: TimelineEntry {
    let date: Date
    let snapshot: UsageSnapshot
    let page: BoardPage
    let port: Int
    /// True while a tapped page is held, so the footer can say so.
    var manual: Bool = false
}

/// One 480×480 design, laid out in points as the tile is.  Every measurement is
/// the device's coordinate divided by 480, so the proportions match the panel.
struct BoardView: View {
    let entry: BoardEntry
    /// The offscreen checks set this; a real widget reads it from the environment.
    var familyOverride: WidgetFamily? = nil
    /// `ImageRenderer` cannot draw a `Link` — it swaps the whole subtree for a
    /// "not allowed" glyph — so the QA renders draw the controls inert.  A link
    /// adds no chrome of its own, so the pictures still show the real layout.
    var interactive: Bool = true
    @Environment(\.colorScheme) private var scheme
    @Environment(\.widgetRenderingMode) private var renderingMode

    var body: some View {
        GeometryReader { geo in
            let s = min(geo.size.width, geo.size.height) / 480
            let palette = BoardPalette.of(scheme, mode: renderingMode)
            VStack(alignment: .leading, spacing: 0) {
                header(s, palette)
                pageTitle(s, palette)
                body(s, palette).padding(.top, 9 * s)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 22 * s)
            .padding(.top, 16 * s)
            // The footer is pinned to the bottom edge: controls must not travel
            // when the pages above it differ in height.
            .padding(.bottom, Self.footerHeight(s) + 13 * s)
            .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
            .overlay(alignment: .bottom) {
                footer(s, palette)
                    .padding(.horizontal, 22 * s)
                    .padding(.bottom, 13 * s)
            }
            .containerBackground(for: .widget) { palette.background }
        }
    }

    /// Button row plus its top gap, reserved out of the page area.
    private static func footerHeight(_ s: CGFloat) -> CGFloat { 22 + 5 * s }

    // MARK: chrome

    private func header(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        HStack(spacing: 6 * s) {
            Circle()
                .fill(stateTint(palette))
                .frame(width: 6 * s, height: 6 * s)
            Text("MAGPIE")
                .font(.system(size: BoardText.brand, weight: .semibold))
                .tracking(0.7)
                .foregroundStyle(palette.ink)
            Spacer(minLength: 0)
            Text(state)
                .font(.system(size: BoardText.status, weight: .medium))
                .foregroundStyle(stateTint(palette))
        }
    }

    private var state: String {
        if !entry.snapshot.available { return "未连接" }
        return entry.snapshot.stale ? "缓存" : "在线"
    }

    /// Green only means "this is live": a disconnected tile is grey, a kept one
    /// is amber, so the dot and the word agree with the numbers under them.
    private func stateTint(_ palette: BoardPalette) -> Color {
        if !entry.snapshot.available { return palette.muted }
        return entry.snapshot.stale ? palette.amber : palette.accent
    }

    private func pageTitle(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 7 * s) {
            HStack(alignment: .firstTextBaseline) {
                Text(entry.page.title)
                    .font(.system(size: BoardText.pageTitle, weight: .semibold))
                    .foregroundStyle(palette.accent)
                Spacer(minLength: 0)
                Text(entry.page.range)
                    .font(.system(size: BoardText.range, weight: .medium))
                    .foregroundStyle(palette.muted)
            }
            Rectangle().fill(palette.divider).frame(height: max(0.5, 1 * s))
        }
        .padding(.top, 9 * s)
    }

    /// Dots and the page count on the left, the controls on the right.  A held
    /// page also says 手动, so the tile never looks like it stopped by itself.
    private func footer(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                ForEach(BoardPage.allCases, id: \.rawValue) { page in
                    Circle()
                        .fill(page == entry.page ? palette.accent : palette.muted.opacity(0.35))
                        .frame(width: 5, height: 5)
                }
            }
            Text("\(entry.page.rawValue + 1)/\(BoardPage.allCases.count)")
                .font(.system(size: BoardText.footer, weight: .medium).monospacedDigit())
                .foregroundStyle(palette.muted)
            if entry.manual {
                Text("手动")
                    .font(.system(size: BoardText.footer, weight: .medium))
                    .foregroundStyle(palette.amber)
            }
            Spacer(minLength: 6)
            control("chevron.left", palette, label: "上一页",
                    url: AICCWidgetMetadata.boardStepURL(-1))
            control("chevron.right", palette, label: "下一页",
                    url: AICCWidgetMetadata.boardStepURL(1))
            control("arrow.clockwise", palette, label: "立即刷新",
                    url: AICCWidgetMetadata.refreshURL)
        }
        .padding(.top, 5 * s)
    }

    /// A footer control.  It is a `Link`, not `Button(intent:)`: the intent path
    /// needs App Intents metadata that only a signed Xcode build produces, and a
    /// hand-written copy is dropped by the system without a word — the button
    /// draws and the tap dies.  A link always works; the price is that the host
    /// app comes forward for a moment.
    @ViewBuilder
    private func control(_ symbol: String, _ palette: BoardPalette,
                         label: String, url: URL) -> some View {
        if interactive {
            Link(destination: url) {
                controlGlyph(symbol, palette)
            }
            .accessibilityLabel(Text(label))
        } else {
            controlGlyph(symbol, palette)
        }
    }

    private func controlGlyph(_ symbol: String, _ palette: BoardPalette) -> some View {
        Image(systemName: symbol)
            .font(.system(size: 10, weight: .semibold))
            .foregroundStyle(palette.ink.opacity(0.9))
            .frame(width: 22, height: 22)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(palette.edge, lineWidth: 1)
            }
    }

    // MARK: pages

    @ViewBuilder
    private func body(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        switch entry.page {
        case .overview: overview(s, palette)
        case .requests: requests(s, palette)
        case .sessions: sessions(s, palette)
        case .context: context(s, palette)
        }
    }

    private func overview(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 15 * s) {
                RingView(permille: entry.snapshot.successPermille,
                         caption: "成功率", s: s, palette: palette,
                         empty: !entry.snapshot.available)
                    .frame(width: 222 * s, height: 222 * s)
                VStack(spacing: 10 * s) {
                    HStack(spacing: 9 * s) {
                        card("调用", UsageFormat.grouped(entry.snapshot.callsToday), palette, s)
                        card("错误", "\(entry.snapshot.errorsToday)", palette, s, tint: palette.amber)
                    }
                    HStack(spacing: 9 * s) {
                        card("剩余额度", UsageFormat.percent(entry.snapshot.remainingPercent), palette, s)
                        card("今日 Token", UsageFormat.count(entry.snapshot.tokensToday), palette, s)
                    }
                }
            }
            .frame(maxHeight: .infinity, alignment: .center)
            summary("7 天 " + UsageFormat.count(entry.snapshot.tokens7d),
                    "30 天 " + UsageFormat.grouped(entry.snapshot.totalCalls) + " 次",
                    UsageFormat.money(micros: entry.snapshot.costMicros), palette, s)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    /// One muted line along the bottom of a page: the numbers that do not earn
    /// a card of their own.
    private func summary(_ leading: String, _ middle: String, _ trailing: String,
                         _ palette: BoardPalette, _ s: CGFloat) -> some View {
        HStack(spacing: 8 * s) {
            Text(leading)
            Spacer(minLength: 0)
            Text(middle)
            Spacer(minLength: 0)
            Text(trailing)
        }
        .font(.system(size: BoardText.summary, weight: .medium))
        .foregroundStyle(palette.muted)
        .lineLimit(1)
        .minimumScaleFactor(0.7)
        .padding(.top, 9 * s)
        .overlay(alignment: .top) {
            Rectangle().fill(palette.divider).frame(height: max(0.5, 1 * s))
        }
    }

    private func requests(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 9 * s) {
            VStack(alignment: .leading, spacing: 3 * s) {
                Text("最近模型")
                    .font(.system(size: BoardText.section, weight: .medium))
                    .foregroundStyle(palette.muted)
                Text(entry.snapshot.latestModel.isEmpty ? "—" : entry.snapshot.latestModel)
                    .font(.system(size: BoardText.cardValue, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }
            VStack(spacing: 9 * s) {
                HStack(spacing: 9 * s) {
                    card("状态", statusText, palette, s,
                         tint: entry.snapshot.latestFailed ? palette.red : palette.accent)
                    card("延迟", UsageFormat.seconds(millis: entry.snapshot.latestLatencyMillis),
                         palette, s)
                }
                HStack(spacing: 9 * s) {
                    card("平均 TTFT", UsageFormat.seconds(millis: entry.snapshot.averageTTFTMillis),
                         palette, s)
                    card("今日 Token", UsageFormat.count(entry.snapshot.tokensToday),
                         palette, s)
                }
            }
            if !entry.snapshot.recent.isEmpty {
                VStack(spacing: 0) {
                    ForEach(Array(entry.snapshot.recent.enumerated()), id: \.offset) { index, request in
                        HStack(spacing: 6 * s) {
                            Text(request.model.isEmpty ? "—" : request.model)
                                .lineLimit(1)
                                .minimumScaleFactor(0.7)
                            Spacer(minLength: 0)
                            Text(request.status == 0 ? "—" : "\(request.status)")
                                .monospacedDigit()
                                .foregroundStyle(request.status >= 400 || request.status < 200
                                                 ? palette.red : palette.muted)
                            Text(UsageFormat.seconds(millis: request.millis))
                                .monospacedDigit()
                                .foregroundStyle(palette.muted)
                                .frame(width: 44, alignment: .trailing)
                        }
                        .font(.system(size: BoardText.row))
                        .foregroundStyle(palette.ink)
                        .padding(.vertical, 4 * s)
                        if index < entry.snapshot.recent.count - 1 { divider(palette, s) }
                    }
                }
                .padding(.top, 2 * s)
            }
            Spacer(minLength: 4 * s)
            summary("今日 " + UsageFormat.grouped(entry.snapshot.callsToday) + " 次",
                    "错误 " + "\(entry.snapshot.errorsToday)",
                    "7 天 " + UsageFormat.count(entry.snapshot.tokens7d), palette, s)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var statusText: String {
        entry.snapshot.latestStatus == 0 ? "—" : "\(entry.snapshot.latestStatus)"
    }

    private func sessions(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 2 * s) {
                Text("累计会话")
                    .font(.system(size: BoardText.section, weight: .medium))
                    .foregroundStyle(palette.muted)
                Text(UsageFormat.grouped(entry.snapshot.sessionCount))
                    .font(.system(size: BoardText.hero, weight: .semibold).monospacedDigit())
                    .foregroundStyle(palette.ink)
            }
            HStack(spacing: 9 * s) {
                card("总 Token", UsageFormat.count(entry.snapshot.totalTokens), palette, s)
                card("估算费用", UsageFormat.money(micros: entry.snapshot.costMicros), palette, s)
            }
            .padding(.top, 12 * s)
            Spacer(minLength: 6 * s)
            VStack(alignment: .leading, spacing: 3 * s) {
                Text("最常用模型")
                    .font(.system(size: BoardText.section, weight: .medium))
                    .foregroundStyle(palette.muted)
                Text(entry.snapshot.topModel.isEmpty ? "—" : entry.snapshot.topModel)
                    .font(.system(size: BoardText.model, weight: .medium))
                    .foregroundStyle(palette.accent)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }
            summary("最常用 " + UsageFormat.grouped(entry.snapshot.topModelCalls) + " 次",
                    "总计 " + UsageFormat.count(entry.snapshot.totalTokens),
                    "", palette, s)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func context(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 11 * s) {
            VStack(alignment: .leading, spacing: 2 * s) {
                Text("缓存命中")
                    .font(.system(size: BoardText.section, weight: .medium))
                    .foregroundStyle(palette.muted)
                HStack(alignment: .firstTextBaseline, spacing: 2 * s) {
                    Text(UsageFormat.tenths(entry.snapshot.cacheHitPermille))
                        .font(.system(size: BoardText.heroFlat, weight: .semibold).monospacedDigit())
                        .foregroundStyle(palette.ink)
                    Text("%")
                        .font(.system(size: BoardText.model, weight: .medium))
                        .foregroundStyle(palette.muted)
                }
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(palette.track)
                    Capsule()
                        .fill(palette.cyan)
                        .frame(width: max(3, geo.size.width * CGFloat(entry.snapshot.cacheHitPermille) / 1000))
                }
            }
            .frame(height: 9 * s)
            VStack(spacing: 0) {
                row("输入", UsageFormat.count(entry.snapshot.totalInput), palette, s, flexible: true)
                divider(palette, s)
                row("输出", UsageFormat.count(entry.snapshot.totalOutput), palette, s, flexible: true)
                divider(palette, s)
                row("缓存读", UsageFormat.count(entry.snapshot.totalCacheRead), palette, s,
                    tint: palette.cyan, flexible: true)
                divider(palette, s)
                row("推理", UsageFormat.count(entry.snapshot.totalReasoning), palette, s, flexible: true)
            }
            .frame(maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    // MARK: pieces

    @ViewBuilder
    private func card(_ label: String, _ value: String, _ palette: BoardPalette,
                      _ s: CGFloat, tint: Color? = nil, flexible: Bool = false) -> some View {
        let body = VStack(alignment: .leading, spacing: 3 * s) {
            Text(label)
                .font(.system(size: BoardText.cardLabel, weight: .medium))
                .foregroundStyle(palette.muted)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
            Spacer(minLength: 0)
            Text(value)
                .font(.system(size: BoardText.cardValue, weight: .semibold).monospacedDigit())
                .foregroundStyle(tint ?? palette.ink)
                .lineLimit(1)
                .minimumScaleFactor(0.6)
        }
        .padding(.horizontal, 9 * s)
        .padding(.vertical, 7 * s)
        let shape = RoundedRectangle(cornerRadius: 8 * s, style: .continuous)
        if flexible {
            body
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                .background(palette.card, in: shape)
                .overlay { shape.strokeBorder(palette.edge, lineWidth: 1) }
        } else {
            body
                .frame(maxWidth: .infinity, minHeight: 74 * s, maxHeight: 74 * s, alignment: .leading)
                .background(palette.card, in: shape)
                .overlay { shape.strokeBorder(palette.edge, lineWidth: 1) }
        }
    }

    @ViewBuilder
    private func row(_ label: String, _ value: String, _ palette: BoardPalette,
                     _ s: CGFloat, tint: Color? = nil, flexible: Bool = false) -> some View {
        let content = HStack {
            Text(label)
                .font(.system(size: 12.5, weight: .medium))
                .foregroundStyle(palette.ink)
            Spacer(minLength: 0)
            Text(value)
                .font(.system(size: 13, weight: .medium).monospacedDigit())
                .foregroundStyle(tint ?? palette.ink)
        }
        .padding(.vertical, 9 * s)
        if flexible {
            content.frame(maxHeight: .infinity)
        } else {
            content
        }
    }

    private func divider(_ palette: BoardPalette, _ s: CGFloat) -> some View {
        Rectangle().fill(palette.divider).frame(height: max(0.5, 1 * s))
    }
}

/// Success rate as the device draws it: a ring with the number inside.
struct RingView: View {
    let permille: Int
    let caption: String
    let s: CGFloat
    let palette: BoardPalette
    /// Nothing is being reported, so an empty ring instead of a 100% that the
    /// tile never measured.
    var empty: Bool = false

    var body: some View {
        ZStack {
            Circle()
                .stroke(palette.track, lineWidth: 13 * s)
            if !empty {
                Circle()
                    .trim(from: 0, to: max(0.001, min(1, CGFloat(permille) / 1000)))
                    .stroke(palette.accent, style: StrokeStyle(lineWidth: 13 * s, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
            VStack(spacing: 2 * s) {
                HStack(alignment: .firstTextBaseline, spacing: 1 * s) {
                    Text(empty ? "—" : UsageFormat.tenths(permille))
                        .font(.system(size: 30, weight: .semibold).monospacedDigit())
                        .foregroundStyle(palette.ink)
                    if !empty {
                        Text("%")
                            .font(.system(size: 14, weight: .medium))
                            .foregroundStyle(palette.muted)
                    }
                }
                Text(caption)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(palette.muted)
            }
        }
        .padding(7 * s)
    }
}

extension UsageSnapshot {
    /// Everything the provider moved, in the sense the panel means by "token".
    var totalTokens: Int { totalInput + totalOutput + totalCacheRead + totalReasoning }
}
