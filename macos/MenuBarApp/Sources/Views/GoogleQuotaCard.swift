import Foundation
import SwiftUI

struct GoogleQuotaCard: View {
    @EnvironmentObject private var settings: AppSettings

    let quota: GoogleQuota?
    let state: OCXGoogleQuotaState

    var body: some View {
        VStack(spacing: 8) {
            HStack {
                Text(settings.localized("Google"))
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                Spacer()
                if state == .stale {
                    Text(settings.localized("Cached"))
                        .font(.system(size: DashboardTypography.timestamp))
                        .foregroundColor(.orange)
                }
            }

            if let quota {
                quotaRow(label: "Gem", window: quota.gem)
                quotaRow(label: "Cla", window: quota.cla)
            } else {
                Text(settings.localized(emptyMessage))
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity)
    }

    private func quotaRow(label: String, window: OCXProviderQuotaWindow?) -> some View {
        VStack(spacing: 4) {
            HStack(alignment: .lastTextBaseline) {
                Text(settings.localized(label))
                    .font(.system(size: DashboardTypography.metricLabel))
                    .foregroundColor(.secondary)
                Spacer()
                if let remaining = window?.remainingPercent {
                    Text(String(format: "%.0f%%", remaining))
                        .font(.system(size: DashboardTypography.secondaryMetric, weight: .semibold))
                        .foregroundColor(progressColor(remaining))
                } else {
                    Text("—")
                        .font(.system(size: DashboardTypography.secondaryMetric, weight: .semibold))
                        .foregroundColor(.secondary)
                }
            }

            if let remaining = window?.remainingPercent {
                progressBar(remaining)
            }

            if let resetAt = window?.resetAt {
                Text(resetText(resetAt))
                    .font(.system(size: DashboardTypography.timestamp))
                    .foregroundColor(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func progressBar(_ value: Double) -> some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 3)
                    .fill(Color.primary.opacity(0.1))
                    .frame(height: 6)
                RoundedRectangle(cornerRadius: 3)
                    .fill(progressColor(value))
                    .frame(width: geo.size.width * CGFloat(min(max(value, 0), 100) / 100), height: 6)
            }
        }
        .frame(height: 6)
    }

    private func resetText(_ date: Date) -> String {
        guard date > Date() else { return settings.localized("Resetting") }
        let formatter = RelativeDateTimeFormatter()
        formatter.locale = settings.locale
        formatter.unitsStyle = .full
        return String(
            format: settings.localized("Resets %@"),
            formatter.localizedString(for: date, relativeTo: Date())
        )
    }

    private var emptyMessage: String {
        switch state {
        case .loading: return "Checking..."
        case .stopped: return "OpenCodex is not running"
        case .notInstalled: return "OpenCodex is not installed"
        case .unavailable: return "Temporarily unavailable"
        default: return "No data"
        }
    }

    private func progressColor(_ value: Double) -> Color {
        if value > 70 { return .green }
        if value >= 30 { return .yellow }
        return .red
    }
}
