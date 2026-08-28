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

    let entry: AICCWidgetEntry

    var body: some View {
        Group {
            if family == .systemMedium {
                mediumContent
            } else {
                smallContent
            }
        }
        .containerBackground(.fill.tertiary, for: .widget)
    }

    private var smallContent: some View {
        VStack(alignment: .leading, spacing: 8) {
            header

            WidgetMetric(title: "Codex", value: entry.snapshot.codex)
            WidgetMetric(title: "WorkBuddy", value: entry.snapshot.workbuddy)

            Spacer(minLength: 0)
        }
        .padding()
    }

    private var mediumContent: some View {
        VStack(alignment: .leading, spacing: 8) {
            header

            HStack(spacing: 12) {
                WidgetMetric(title: "CODEX", value: entry.snapshot.codex)
                WidgetMetric(title: "WORKBUDDY", value: entry.snapshot.workbuddy)
            }

            HStack(spacing: 12) {
                WidgetMetric(title: "DEEPSEEK", value: entry.snapshot.deepseek)
                WidgetMetric(title: "SYSTEM", value: entry.snapshot.system)
            }
        }
        .padding()
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text("AICC")
                .font(.headline)
            Spacer(minLength: 8)
            Button(intent: RefreshWidgetIntent()) {
                Image(systemName: "arrow.clockwise")
                    .font(.caption.weight(.semibold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .accessibilityLabel("Refresh Widget")
        }
    }
}

private struct WidgetMetric: View {
    let title: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Text(value)
                .font(.title3.weight(.semibold).monospacedDigit())
                .lineLimit(1)
                .minimumScaleFactor(0.65)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
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
