import SwiftUI
import WidgetKit

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
                                accented: true,
                                isDark: true)
        }
        return scheme == .dark
            ? BoardPalette(background: Color(red: 0x0B / 255, green: 0x12 / 255, blue: 0x10 / 255),
                           card: Color(red: 0x12 / 255, green: 0x1A / 255, blue: 0x16 / 255),
                           track: Color(red: 0x1A / 255, green: 0x23 / 255, blue: 0x1E / 255),
                           divider: Color(red: 0x1E / 255, green: 0x2A / 255, blue: 0x25 / 255),
                           ink: Color(red: 0xF4 / 255, green: 0xF7 / 255, blue: 0xED / 255),
                           muted: Color(red: 0x71 / 255, green: 0x80 / 255, blue: 0x79 / 255),
                           accent: Color(red: 0xA8 / 255, green: 0xFF / 255, blue: 0x4F / 255),
                           amber: Color(red: 0xFF / 255, green: 0xB0 / 255, blue: 0x2E / 255),
                           red: Color(red: 0xFF / 255, green: 0x5F / 255, blue: 0x57 / 255),
                           cyan: Color(red: 0x55 / 255, green: 0xE6 / 255, blue: 0xD2 / 255),
                           isDark: true)
            : BoardPalette(background: Color(red: 0xF7 / 255, green: 0xF8 / 255, blue: 0xF3 / 255),
                           card: .white,
                           track: Color(red: 0xE4 / 255, green: 0xE6 / 255, blue: 0xDE / 255),
                           divider: Color(red: 0xD9 / 255, green: 0xDC / 255, blue: 0xD2 / 255),
                           ink: Color(red: 0x14 / 255, green: 0x17 / 255, blue: 0x0F / 255),
                           muted: Color(red: 0x6A / 255, green: 0x6F / 255, blue: 0x63 / 255),
                           accent: Color(red: 0x4E / 255, green: 0x8C / 255, blue: 0x0E / 255),
                           amber: Color(red: 0xA9 / 255, green: 0x69 / 255, blue: 0x0A / 255),
                           red: Color(red: 0xC0 / 255, green: 0x39 / 255, blue: 0x2B / 255),
                           cyan: Color(red: 0x0E / 255, green: 0x8C / 255, blue: 0x7C / 255),
                           isDark: false)
    }
}

struct BoardEntry: TimelineEntry {
    let date: Date
    let snapshot: UsageSnapshot
    let page: BoardPage
    let port: Int
}

/// One 480×480 design, laid out in points as the tile is.  Every measurement is
/// the device's coordinate divided by 480, so the proportions match the panel.
struct BoardView: View {
    let entry: BoardEntry
    /// The offscreen checks set this; a real widget reads it from the environment.
    var familyOverride: WidgetFamily? = nil
    @Environment(\.colorScheme) private var scheme
    @Environment(\.widgetRenderingMode) private var renderingMode

    var body: some View {
        GeometryReader { geo in
            let s = min(geo.size.width, geo.size.height) / 480
            let palette = BoardPalette.of(scheme, mode: renderingMode)
            VStack(alignment: .leading, spacing: 0) {
                header(s, palette)
                pageTitle(s, palette)
                body(s, palette).padding(.top, 10 * s)
                Spacer(minLength: 0)
                footer(s, palette)
            }
            .padding(.horizontal, 22 * s)
            .padding(.top, 16 * s)
            .padding(.bottom, 14 * s)
            .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
            .containerBackground(for: .widget) { palette.background }
        }
    }

    // MARK: chrome

    private func header(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        HStack(spacing: 6 * s) {
            Circle()
                .fill(entry.snapshot.stale ? palette.amber : palette.accent)
                .frame(width: 6 * s, height: 6 * s)
            Text("MAGPIE")
                .font(.system(size: fs(14, s), weight: .medium))
                .foregroundStyle(palette.ink)
            Spacer(minLength: 0)
            Text(state)
                .font(.system(size: fs(12, s), weight: .medium))
                .foregroundStyle(entry.snapshot.stale ? palette.amber : palette.accent)
        }
    }

    private var state: String {
        if !entry.snapshot.available { return "未连接" }
        return entry.snapshot.stale ? "缓存" : "在线"
    }

    private func pageTitle(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 8 * s) {
            HStack(alignment: .firstTextBaseline) {
                Text(entry.page.title)
                    .font(.system(size: fs(20, s), weight: .semibold))
                    .foregroundStyle(palette.accent)
                Spacer(minLength: 0)
                Text(entry.page.range)
                    .font(.system(size: fs(12, s)))
                    .foregroundStyle(palette.muted)
            }
            Rectangle().fill(palette.divider).frame(height: max(0.5, 1 * s))
        }
        .padding(.top, 10 * s)
    }

    private func footer(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        HStack(spacing: 8 * s) {
            ForEach(BoardPage.allCases, id: \.rawValue) { page in
                Circle()
                    .fill(page == entry.page ? palette.accent : palette.muted.opacity(0.35))
                    .frame(width: 6 * s, height: 6 * s)
            }
            Spacer(minLength: 0)
            Text("\(entry.page.rawValue + 1)/\(BoardPage.allCases.count)")
                .font(.system(size: fs(11, s)))
                .foregroundStyle(palette.muted)
        }
        .padding(.top, 6 * s)
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
            HStack(alignment: .center, spacing: 18 * s) {
                RingView(permille: entry.snapshot.successPermille,
                         caption: "成功率", s: s, palette: palette)
                    .frame(width: 224 * s, height: 224 * s)
                VStack(spacing: 10 * s) {
                    HStack(spacing: 10 * s) {
                        card("调用", UsageFormat.grouped(entry.snapshot.callsToday), palette, s)
                        card("错误", "\(entry.snapshot.errorsToday)", palette, s, tint: palette.amber)
                    }
                    HStack(spacing: 10 * s) {
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
        .font(.system(size: fs(12, s)))
        .foregroundStyle(palette.muted)
        .lineLimit(1)
        .minimumScaleFactor(0.7)
        .padding(.top, 10 * s)
        .overlay(alignment: .top) {
            Rectangle().fill(palette.divider).frame(height: max(0.5, 1 * s))
        }
    }

    private func requests(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 14 * s) {
            VStack(alignment: .leading, spacing: 4 * s) {
                Text("最近模型")
                    .font(.system(size: fs(12, s)))
                    .foregroundStyle(palette.muted)
                Text(entry.snapshot.latestModel.isEmpty ? "—" : entry.snapshot.latestModel)
                    .font(.system(size: fs(24, s), weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }
            VStack(spacing: 10 * s) {
                HStack(spacing: 10 * s) {
                    card("状态", statusText, palette, s,
                         tint: entry.snapshot.latestFailed ? palette.red : palette.accent)
                    card("延迟", UsageFormat.seconds(millis: entry.snapshot.latestLatencyMillis),
                         palette, s)
                }
                HStack(spacing: 10 * s) {
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
                                .foregroundStyle(request.status >= 400 || request.status < 200
                                                 ? palette.red : palette.muted)
                            Text(UsageFormat.seconds(millis: request.millis))
                                .foregroundStyle(palette.muted)
                                .frame(width: 46 * s, alignment: .trailing)
                        }
                        .font(.system(size: fs(12, s)))
                        .foregroundStyle(palette.ink)
                        .padding(.vertical, 6 * s)
                        if index < entry.snapshot.recent.count - 1 { divider(palette, s) }
                    }
                }
                .padding(.top, 4 * s)
            }
            Spacer(minLength: 6 * s)
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
        VStack(alignment: .leading, spacing: 14 * s) {
            VStack(alignment: .leading, spacing: 2 * s) {
                Text("会话")
                    .font(.system(size: fs(12, s)))
                    .foregroundStyle(palette.muted)
                Text(UsageFormat.grouped(entry.snapshot.sessionCount))
                    .font(.system(size: fs(54, s), weight: .semibold))
                    .foregroundStyle(palette.ink)
            }
            HStack(spacing: 10 * s) {
                card("总 Token", UsageFormat.count(entry.snapshot.totalTokens), palette, s)
                card("估算费用", UsageFormat.money(micros: entry.snapshot.costMicros), palette, s)
            }
            Spacer(minLength: 6 * s)
            VStack(alignment: .leading, spacing: 4 * s) {
                Text("最常用模型")
                    .font(.system(size: fs(12, s)))
                    .foregroundStyle(palette.muted)
                Text(entry.snapshot.topModel.isEmpty ? "—" : entry.snapshot.topModel)
                    .font(.system(size: fs(20, s), weight: .medium))
                    .foregroundStyle(palette.accent)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }
            Spacer(minLength: 0)
            summary("最常用 " + UsageFormat.grouped(entry.snapshot.topModelCalls) + " 次",
                    "总计 " + UsageFormat.count(entry.snapshot.totalTokens),
                    "", palette, s)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func context(_ s: CGFloat, _ palette: BoardPalette) -> some View {
        VStack(alignment: .leading, spacing: 12 * s) {
            VStack(alignment: .leading, spacing: 2 * s) {
                Text("缓存命中")
                    .font(.system(size: fs(12, s)))
                    .foregroundStyle(palette.muted)
                HStack(alignment: .firstTextBaseline, spacing: 2 * s) {
                    Text(UsageFormat.tenths(entry.snapshot.cacheHitPermille))
                        .font(.system(size: fs(50, s), weight: .semibold))
                        .foregroundStyle(palette.ink)
                    Text("%")
                        .font(.system(size: fs(20, s), weight: .medium))
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
            .frame(height: 10 * s)
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
        let body = VStack(alignment: .leading, spacing: 4 * s) {
            Text(label)
                .font(.system(size: fs(12, s)))
                .foregroundStyle(palette.muted)
                .lineLimit(1)
            Spacer(minLength: 0)
            Text(value)
                .font(.system(size: fs(22, s), weight: .semibold))
                .foregroundStyle(tint ?? palette.ink)
                .lineLimit(1)
                .minimumScaleFactor(0.6)
        }
        .padding(.horizontal, 10 * s)
        .padding(.vertical, 8 * s)
        if flexible {
            body
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 8 * s, style: .continuous))
        } else {
            body
                .frame(maxWidth: .infinity, minHeight: 78 * s, maxHeight: 78 * s, alignment: .leading)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 8 * s, style: .continuous))
        }
    }

    @ViewBuilder
    private func row(_ label: String, _ value: String, _ palette: BoardPalette,
                     _ s: CGFloat, tint: Color? = nil, flexible: Bool = false) -> some View {
        let content = HStack {
            Text(label)
                .font(.system(size: fs(13, s), weight: .medium))
                .foregroundStyle(palette.ink)
            Spacer(minLength: 0)
            Text(value)
                .font(.system(size: fs(14, s), weight: .medium).monospacedDigit())
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

    private func fs(_ size: CGFloat, _ s: CGFloat) -> CGFloat { max(9, size * s) }
}

/// Success rate as the device draws it: a ring with the number inside.
struct RingView: View {
    let permille: Int
    let caption: String
    let s: CGFloat
    let palette: BoardPalette

    var body: some View {
        ZStack {
            Circle()
                .stroke(palette.track, lineWidth: 14 * s)
            Circle()
                .trim(from: 0, to: max(0.001, min(1, CGFloat(permille) / 1000)))
                .stroke(palette.accent, style: StrokeStyle(lineWidth: 14 * s, lineCap: .round))
                .rotationEffect(.degrees(-90))
            VStack(spacing: 2 * s) {
                HStack(alignment: .firstTextBaseline, spacing: 1 * s) {
                    Text(UsageFormat.tenths(permille))
                        .font(.system(size: max(14, 42 * s), weight: .semibold))
                        .foregroundStyle(palette.ink)
                    Text("%")
                        .font(.system(size: max(9, 16 * s), weight: .medium))
                        .foregroundStyle(palette.muted)
                }
                Text(caption)
                    .font(.system(size: max(9, 11 * s)))
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
