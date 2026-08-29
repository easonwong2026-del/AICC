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
            completion(
                Timeline(
                    entries: [entry],
                    policy: .after(now.addingTimeInterval(15 * 60))
                )
            )
        }
    }
}

struct AICCWidgetView: View {
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

    // MARK: - Medium Widget

    private var mediumContent: some View {
        VStack(alignment: .leading, spacing: 3.5) {
            header

            codexMainCard

            HStack(spacing: 6) {
                workbuddyCard
                deepseekCard
            }
            .frame(maxWidth: .infinity)

            footer
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
    }

    // MARK: - Small Widget

    private var smallContent: some View {
        VStack(alignment: .leading, spacing: 4) {
            header

            smallCodexCard

            smallWorkBuddyCard

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
    }

    // MARK: - Shared Header

    private var header: some View {
        HStack(alignment: .center) {
            Text("AICC")
                .font(.system(size: 12, weight: .bold))
                .foregroundStyle(.primary)

            Spacer(minLength: 8)

            Button(intent: RefreshWidgetIntent()) {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 10.5, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Refresh Widget")
        }
    }

    // MARK: - Medium Components

    private var codexMainCard: some View {
        VStack(alignment: .leading, spacing: 2.5) {
            Text("Codex 每周额度")
                .font(.system(size: 9.5, weight: .medium))
                .foregroundStyle(.secondary)

            HStack(alignment: .lastTextBaseline, spacing: 1.5) {
                Text(entry.snapshot.codexWeeklyNumber)
                    .font(.system(size: 25, weight: .bold, design: .rounded).monospacedDigit())
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)

                if entry.snapshot.codexWeeklyNumber != "—" {
                    Text("%")
                        .font(.system(size: 13, weight: .semibold, design: .rounded))
                        .foregroundStyle(.primary)
                }

                Spacer(minLength: 8)
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(Color.primary.opacity(0.12))
                        .frame(height: 4.5)

                    Capsule()
                        .fill(
                            LinearGradient(
                                colors: [Color.green, Color.mint],
                                startPoint: .leading,
                                endPoint: .trailing
                            )
                        )
                        .frame(
                            width: max(0, min(geo.size.width * CGFloat(entry.snapshot.codexWeeklyProgress), geo.size.width)),
                            height: 4.5
                        )
                }
            }
            .frame(height: 4.5)

            HStack(alignment: .center) {
                if let resetText = entry.snapshot.codexResetText {
                    HStack(spacing: 2.5) {
                        Image(systemName: "clock")
                            .font(.system(size: 8))
                        Text(resetText)
                            .font(.system(size: 8.5))
                    }
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                }

                Spacer(minLength: 4)

                if let fiveRem = entry.snapshot.codexFiveHourRemaining {
                    HStack(spacing: 2) {
                        Text("5 小时")
                            .font(.system(size: 8.5))
                            .foregroundStyle(.secondary)
                        Text(String(format: " %.0f%%", fiveRem))
                            .font(.system(size: 8.5, weight: .semibold))
                            .foregroundStyle(.green)
                    }
                    .lineLimit(1)
                }
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(
            RoundedRectangle(cornerRadius: 8)
                .fill(Color.primary.opacity(0.04))
                .background(
                    RoundedRectangle(cornerRadius: 8)
                        .fill(
                            LinearGradient(
                                colors: [Color.green.opacity(0.07), Color.mint.opacity(0.03), Color.clear],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            )
                        )
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 8)
                        .strokeBorder(Color.green.opacity(0.15), lineWidth: 0.8)
                )
        )
    }

    private var workbuddyCard: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 3.5) {
                Circle()
                    .fill(Color.purple)
                    .frame(width: 4, height: 4)
                Text("WorkBuddy")
                    .font(.system(size: 9.5, weight: .medium))
                    .foregroundStyle(Color.purple)
                Spacer(minLength: 0)
            }

            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(entry.snapshot.workbuddyPointsText)
                    .font(.system(size: 16, weight: .bold).monospacedDigit())
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)

                if entry.snapshot.workbuddyPointsText != "—" {
                    Text("积分")
                        .font(.system(size: 8.5, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }

            HStack(spacing: 2.5) {
                Image(systemName: "link")
                    .font(.system(size: 8))
                    .foregroundStyle(entry.snapshot.workbuddyIsOnline ? Color.purple : Color.secondary)
                Text(entry.snapshot.workbuddySubtitle)
                    .font(.system(size: 8.5))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 7)
                .fill(Color.primary.opacity(0.04))
                .background(
                    RoundedRectangle(cornerRadius: 7)
                        .fill(Color.purple.opacity(0.05))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 7)
                        .strokeBorder(Color.purple.opacity(0.15), lineWidth: 0.8)
                )
        )
    }

    private var deepseekCard: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 3.5) {
                Circle()
                    .fill(Color.cyan)
                    .frame(width: 4, height: 4)
                Text("DeepSeek")
                    .font(.system(size: 9.5, weight: .medium))
                    .foregroundStyle(Color.cyan)
                Spacer(minLength: 0)
            }

            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(entry.snapshot.deepseekBalanceText)
                    .font(.system(size: 16, weight: .bold).monospacedDigit())
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)

                if entry.snapshot.deepseekBalanceText != "—" && !entry.snapshot.deepseekCurrency.isEmpty {
                    Text(entry.snapshot.deepseekCurrency)
                        .font(.system(size: 8.5, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }

            HStack(spacing: 2.5) {
                Image(systemName: "link")
                    .font(.system(size: 8))
                    .foregroundStyle(entry.snapshot.deepseekIsOnline ? Color.cyan : Color.secondary)
                Text(entry.snapshot.deepseekStatusText)
                    .font(.system(size: 8.5))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 7)
                .fill(Color.primary.opacity(0.04))
                .background(
                    RoundedRectangle(cornerRadius: 7)
                        .fill(Color.cyan.opacity(0.05))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 7)
                        .strokeBorder(Color.cyan.opacity(0.15), lineWidth: 0.8)
                )
        )
    }

    private var footer: some View {
        HStack(spacing: 2.5) {
            Image(systemName: "clock")
                .font(.system(size: 8))
            Text(entry.snapshot.formattedFooterTime(at: entry.date))
                .font(.system(size: 8.5))
        }
        .foregroundStyle(.secondary)
        .lineLimit(1)
    }

    // MARK: - Small Components

    private var smallCodexCard: some View {
        VStack(alignment: .leading, spacing: 2.5) {
            Text("Codex 每周额度")
                .font(.system(size: 9.5, weight: .medium))
                .foregroundStyle(.secondary)

            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(entry.snapshot.codexWeeklyNumber)
                    .font(.system(size: 24, weight: .bold, design: .rounded).monospacedDigit())
                    .foregroundStyle(.primary)

                if entry.snapshot.codexWeeklyNumber != "—" {
                    Text("%")
                        .font(.system(size: 12, weight: .semibold, design: .rounded))
                        .foregroundStyle(.primary)
                }
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(Color.primary.opacity(0.12))
                        .frame(height: 4.5)

                    Capsule()
                        .fill(
                            LinearGradient(
                                colors: [Color.green, Color.mint],
                                startPoint: .leading,
                                endPoint: .trailing
                            )
                        )
                        .frame(
                            width: max(0, min(geo.size.width * CGFloat(entry.snapshot.codexWeeklyProgress), geo.size.width)),
                            height: 4.5
                        )
                }
            }
            .frame(height: 4.5)
        }
        .padding(6.5)
        .background(
            RoundedRectangle(cornerRadius: 7)
                .fill(Color.primary.opacity(0.04))
                .background(
                    RoundedRectangle(cornerRadius: 7)
                        .fill(Color.green.opacity(0.05))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 7)
                        .strokeBorder(Color.green.opacity(0.12), lineWidth: 0.8)
                )
        )
    }

    private var smallWorkBuddyCard: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 3) {
                Circle()
                    .fill(Color.purple)
                    .frame(width: 3.5, height: 3.5)
                Text("WorkBuddy")
                    .font(.system(size: 8.5, weight: .medium))
                    .foregroundStyle(Color.purple)
            }

            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(entry.snapshot.workbuddyPointsText)
                    .font(.system(size: 13, weight: .bold).monospacedDigit())
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)

                if entry.snapshot.workbuddyPointsText != "—" {
                    Text("积分")
                        .font(.system(size: 7.5, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(.horizontal, 6.5)
        .padding(.vertical, 4.5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 7)
                .fill(Color.primary.opacity(0.04))
                .background(
                    RoundedRectangle(cornerRadius: 7)
                        .fill(Color.purple.opacity(0.05))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 7)
                        .strokeBorder(Color.purple.opacity(0.12), lineWidth: 0.8)
                )
        )
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
    }
}
