import SwiftUI

struct WorkBuddyCard: View {
    @EnvironmentObject private var settings: AppSettings
    let data: WorkBuddyData

    var body: some View {
        CompactCard(
            title: "WorkBuddy",
            icon: "wand.and.stars",
            value: formattedPoints,
            subtitle: statusSubtitle,
            state: state,
            unit: data.points == nil ? nil : settings.localized("Points"),
            valueFontSize: DashboardTypography.primaryFontSize(number: formattedPoints, compact: true)
        )
    }

    private var formattedPoints: String {
        guard let points = data.points else {
            return settings.localized("Temporarily unavailable")
        }
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = true
        formatter.maximumFractionDigits = 2
        formatter.minimumFractionDigits = 0
        return formatter.string(from: NSNumber(value: points)) ?? String(points)
    }

    private var statusSubtitle: String {
        let statusKey = data.balance_state ?? (data.points == nil ? "Unavailable" : "Connected")
        let status = settings.localized(statusKey)
        if data.points == nil {
            if data.balance_error_code == "bridge_unavailable" {
                return settings.localized("Not connected · Settings → reconnect WorkBuddy")
            }
            if let error = data.balance_error {
                return "\(status) · \(error)"
            }
            if let code = data.balance_error_code {
                return "\(status) · \(code)"
            }
        }
        if let age = data.balance_age_seconds {
            return "\(status) · \(ageText(age))"
        }
        if let updated = data.balance_updated_at, !updated.isEmpty {
            return "\(status) · \(updated)"
        }
        return status
    }

    private func ageText(_ seconds: Int) -> String {
        if seconds < 60 { return String(format: settings.localized("%ds ago"), seconds) }
        if seconds < 3600 { return String(format: settings.localized("%dm ago"), seconds / 60) }
        if seconds < 86400 { return String(format: settings.localized("%dh ago"), seconds / 3600) }
        return String(format: settings.localized("%dd ago"), seconds / 86400)
    }

    private var state: CardState {
        if data.balance_stale == true || data.balance_state == "Cached" { return .stale }
        if data.balance_state == "Connected" { return .online }
        if data.points == nil { return .unavailable }
        return .online
    }
}
