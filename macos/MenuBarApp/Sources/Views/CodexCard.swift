import SwiftUI

struct CodexCard: View {
    @EnvironmentObject private var settings: AppSettings
    let codex: CodexData
    let snapshot: WidgetDisplaySnapshot

    var body: some View {
        VStack(spacing: 8) {
            if let weekly = codex.weekly, let remaining = snapshot.codexWeeklyRemaining {
                weeklySection(weekly: weekly, remaining: remaining)
            } else if let fiveHour = codex.five_hour, let remaining = snapshot.codexFiveHourRemaining {
                fiveHourOnlySection(fiveHour: fiveHour, remaining: remaining)
            } else {
                placeholderContent
            }
        }
        .frame(maxWidth: .infinity)
        .overlay(alignment: .topLeading) {
            if snapshot.codexState == "stale" {
                Text("缓存").font(.system(size: 9)).foregroundColor(.orange).offset(y: -9)
            }
        }
    }

    private func weeklySection(weekly: RateWindow, remaining: Double) -> some View {
        VStack(spacing: 6) {
            HStack(alignment: .lastTextBaseline) {
                Text("Codex Weekly")
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                    .foregroundColor(.secondary)
                Spacer()
                quotaNumber(remaining, unit: "%", baseSize: 34)
            }

            progressBar(remaining, height: 6)

            HStack(spacing: 10) {
                if let reset = snapshot.codexWeeklyReset, !reset.isEmpty {
                    Text(resetText(reset))
                        .font(.system(size: DashboardTypography.timestamp))
                        .foregroundColor(.secondary)
                }
                Spacer()
                if let fiveRem = snapshot.codexSecondaryFiveHourRemaining {
                    HStack(alignment: .lastTextBaseline, spacing: 3) {
                        Text("5 Hour")
                            .font(.system(size: DashboardTypography.metricLabel))
                            .foregroundColor(.secondary)
                        Text(String(format: "%.0f", fiveRem))
                            .font(.system(size: DashboardTypography.secondaryMetric, weight: .semibold))
                            .foregroundColor(progressColor(fiveRem))
                        Text("%")
                            .font(.system(size: 10))
                            .foregroundColor(.secondary)
                    }
                }
            }
        }
    }

    private func fiveHourOnlySection(fiveHour: RateWindow, remaining: Double) -> some View {
        VStack(spacing: 6) {
            HStack(alignment: .lastTextBaseline) {
                Text("Codex 5h")
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                    .foregroundColor(.secondary)
                Spacer()
                quotaNumber(remaining, unit: "%", baseSize: 34)
            }
            progressBar(remaining, height: 6)
            if let reset = snapshot.codexWeeklyReset, !reset.isEmpty {
                Text(resetText(reset))
                    .font(.system(size: DashboardTypography.timestamp))
                    .foregroundColor(.secondary)
            }
        }
    }

    private func quotaNumber(_ value: Double, unit: String, baseSize: CGFloat) -> some View {
        let number = String(format: "%.0f", value)
        return HStack(alignment: .lastTextBaseline, spacing: 3) {
            Text(number)
                .font(.system(
                    size: max(baseSize - 2, DashboardTypography.primaryFontSize(number: number)),
                    weight: .bold
                ))
                .foregroundColor(progressColor(value))
                .lineLimit(1)
            Text(unit)
                .font(.system(size: DashboardTypography.unit, weight: .medium))
                .foregroundColor(.secondary)
        }
    }

    private func progressBar(_ value: Double, height: CGFloat) -> some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: height / 2)
                    .fill(Color.primary.opacity(0.1))
                    .frame(height: height)
                RoundedRectangle(cornerRadius: height / 2)
                    .fill(progressColor(value))
                    .frame(width: geo.size.width * CGFloat(min(max(value, 0), 100) / 100.0), height: height)
            }
        }
        .frame(height: height)
    }

    private func resetText(_ reset: String) -> String {
        String(format: settings.localized("Reset %@"), reset)
    }

    private var placeholderContent: some View {
        HStack {
            Image(systemName: "chart.bar.fill")
                .foregroundColor(.secondary)
            Text("Codex: No data")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
    }

    private func progressColor(_ value: Double) -> Color {
        if value > 70 { return .green }
        if value >= 30 { return .yellow }
        return .red
    }
}
