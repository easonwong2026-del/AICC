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

    // MARK: - Medium Widget (Flattened Large-Typography Layout)

    private var mediumContent: some View {
        VStack(alignment: .leading, spacing: 4) {
            header
                .fixedSize(horizontal: false, vertical: true)

            GeometryReader { geo in
                let gap: CGFloat = 14
                let leftWidth = (geo.size.width - gap) * 0.58
                let rightWidth = (geo.size.width - gap) * 0.42

                HStack(alignment: .top, spacing: gap) {
                    codexMainView
                        .frame(width: leftWidth, alignment: .topLeading)

                    VStack(alignment: .leading, spacing: 0) {
                        workbuddyView
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)

                        Divider()
                            .overlay(Color.primary.opacity(0.12))
                            .padding(.vertical, 2)

                        deepseekView
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    }
                    .frame(width: rightWidth, height: geo.size.height)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            footer
                .frame(height: 12, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
    }

    // MARK: - Small Widget (Unchanged)

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
                .font(.system(size: 12.5, weight: .bold))
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

    // MARK: - Medium Left Component (Codex Main View)

    private var codexMainView: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(entry.snapshot.codexTitle)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)

            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(entry.snapshot.codexWeeklyNumber)
                    .font(.system(size: 36, weight: .bold, design: .rounded).monospacedDigit())
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)

                if entry.snapshot.codexWeeklyNumber != "—" {
                    Text("%")
                        .font(.system(size: 18, weight: .semibold, design: .rounded))
                        .foregroundStyle(.primary)
                }

                Spacer(minLength: 0)
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(Color.primary.opacity(0.12))
                        .frame(height: 5.5)

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
                            height: 5.5
                        )
                }
            }
            .frame(height: 5.5)
            .padding(.top, 1)
            .padding(.bottom, 3)

            HStack(alignment: .center, spacing: 4) {
                if let resetText = entry.snapshot.codexResetShortText {
                    Text(resetText)
                        .font(.system(size: 9))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer(minLength: 2)

                if let fiveRem = entry.snapshot.codexSecondaryFiveHourRemaining {
                    HStack(spacing: 1.5) {
                        Text("5小时")
                            .font(.system(size: 9))
                            .foregroundStyle(.secondary)
                        Text(String(format: " %.0f%%", fiveRem))
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(.green)
                    }
                    .lineLimit(1)
                }
            }
            .lineLimit(1)
            .minimumScaleFactor(0.8)
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    // MARK: - Medium Right Top Component (WorkBuddy View)

    private var workbuddyView: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 4) {
                Circle()
                    .fill(Color.purple)
                    .frame(width: 4.5, height: 4.5)
                Text("WorkBuddy")
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(Color.purple)
                Spacer(minLength: 0)
            }

            HStack(alignment: .lastTextBaseline, spacing: 2.5) {
                Text(entry.snapshot.workbuddyPointsText)
                    .font(.system(size: 21, weight: .bold).monospacedDigit())
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)

                if entry.snapshot.workbuddyPointsText != "—" {
                    Text("积分")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }

            Text(entry.snapshot.workbuddySubtitle)
                .font(.system(size: 9.5))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    // MARK: - Medium Right Bottom Component (DeepSeek View)

    private var deepseekView: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 4) {
                Circle()
                    .fill(Color.cyan)
                    .frame(width: 4.5, height: 4.5)
                Text("DeepSeek")
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(Color.cyan)
                Spacer(minLength: 0)
            }

            HStack(alignment: .lastTextBaseline, spacing: 2.5) {
                Text(entry.snapshot.deepseekBalanceText)
                    .font(.system(size: 21, weight: .bold).monospacedDigit())
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)

                if entry.snapshot.deepseekBalanceText != "—" && !entry.snapshot.deepseekCurrency.isEmpty {
                    Text(entry.snapshot.deepseekCurrency)
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }

            Text(entry.snapshot.deepseekStatusText)
                .font(.system(size: 9.5))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    // MARK: - Footer

    private var footer: some View {
        Text(entry.snapshot.formattedFooterTime(at: entry.date))
            .font(.system(size: 9))
            .foregroundStyle(.secondary)
            .lineLimit(1)
    }

    // MARK: - Small Components (Unchanged)

    private var smallCodexCard: some View {
        VStack(alignment: .leading, spacing: 2.5) {
            Text(entry.snapshot.codexTitle)
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
        .contentMarginsDisabled()
    }
}
