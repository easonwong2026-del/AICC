import SwiftUI

struct GoogleQuotaCard: View {
    @EnvironmentObject private var settings: AppSettings
    let snapshot: WidgetDisplaySnapshot

    var body: some View {
        let quota = snapshot.google
        VStack(spacing: 6) {
            HStack(alignment: .lastTextBaseline) {
                Text(settings.localized(quota?.title ?? "Google"))
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                    .foregroundColor(.secondary)
                if snapshot.googleState == "stale" {
                    Text(settings.localized("Cached"))
                        .font(.system(size: DashboardTypography.timestamp))
                        .foregroundColor(.orange)
                }
                Spacer()
                Text(quota?.number ?? "—")
                    .font(.system(size: 34, weight: .bold).monospacedDigit())
                    .foregroundColor(color(quota?.primary?.remaining))
                if quota?.primary != nil {
                    Text("%")
                        .font(.system(size: DashboardTypography.unit, weight: .medium))
                        .foregroundColor(.secondary)
                }
            }
            if let remaining = quota?.primary?.remaining {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.primary.opacity(0.1))
                        Capsule().fill(color(remaining)).frame(width: geo.size.width * remaining / 100)
                    }
                }
                .frame(height: 6)
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    if let reset = quota?.reset {
                        Text(String(format: settings.localized("Reset %@"), reset))
                            .font(.system(size: DashboardTypography.timestamp))
                            .foregroundColor(.secondary)
                    }
                    Spacer(minLength: 0)
                    if let secondary = quota?.secondary?.remaining {
                        Text(settings.localized("5 Hour"))
                            .font(.system(size: DashboardTypography.metricLabel))
                            .foregroundColor(.secondary)
                        Text(String(format: "%.0f%%", secondary))
                            .font(.system(size: DashboardTypography.secondaryMetric, weight: .semibold))
                            .foregroundColor(color(secondary))
                    }
                }
                if quota?.isWeekly == false {
                    Text(settings.localized("Weekly quota unavailable"))
                        .font(.system(size: DashboardTypography.timestamp))
                        .foregroundColor(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                Text(settings.localized(quota?.error ?? "No data"))
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .combine)
    }

    private func color(_ value: Double?) -> Color {
        guard let value else { return .secondary }
        return value > 70 ? .green : (value >= 30 ? .yellow : .red)
    }
}
