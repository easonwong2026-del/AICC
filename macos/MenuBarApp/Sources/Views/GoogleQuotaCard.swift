import SwiftUI

struct GoogleQuotaCard: View {
    @EnvironmentObject private var settings: AppSettings
    let snapshot: WidgetDisplaySnapshot

    var body: some View {
        let quota = snapshot.google
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline) {
                Text("Google")
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                    .foregroundColor(.secondary)
                if snapshot.googleState == "stale" {
                    Text(settings.localized("Cached"))
                        .font(.system(size: DashboardTypography.timestamp))
                        .foregroundColor(.orange)
                }
                Spacer()
                if let reset = quota?.reset, !reset.isEmpty {
                    let shortReset = reset.replacingOccurrences(of: "^\\d{4}-", with: "", options: .regularExpression)
                    Text("Reset " + shortReset)
                        .font(.system(size: DashboardTypography.timestamp))
                        .foregroundColor(.secondary)
                }
            }

            if quota?.primary != nil {
                compactQuotaRow(label: "Weekly", window: quota?.weekly)
                compactQuotaRow(label: "5h", window: quota?.five_hour)
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

    private func compactQuotaRow(label: String, window: WidgetRateWindow?) -> some View {
        let remaining = window?.remaining
        let isKnown = remaining != nil && remaining!.isFinite
        let value = isKnown ? remaining! : 0.0

        return HStack(spacing: 8) {
            Text(label)
                .font(.system(size: 11, weight: .medium))
                .foregroundColor(.secondary)
                .frame(width: 44, alignment: .leading)

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(Color.primary.opacity(0.1))
                        .frame(height: 4)
                    if isKnown {
                        Capsule()
                            .fill(color(value))
                            .frame(width: geo.size.width * CGFloat(min(max(value, 0), 100) / 100.0), height: 4)
                    }
                }
            }
            .frame(height: 4)

            Text(isKnown ? String(format: "%.0f%%", value) : "—")
                .font(.system(size: 11, weight: .semibold).monospacedDigit())
                .foregroundColor(isKnown ? color(value) : .secondary)
                .frame(width: 38, alignment: .trailing)
        }
    }

    private func color(_ value: Double?) -> Color {
        guard let value else { return .secondary }
        return value > 70 ? .green : (value >= 30 ? .yellow : .red)
    }
}
