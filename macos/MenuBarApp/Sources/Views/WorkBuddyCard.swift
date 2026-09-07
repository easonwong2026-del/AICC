import SwiftUI

struct WorkBuddyCard: View {
    @EnvironmentObject private var settings: AppSettings
    let data: WorkBuddyData
    let snapshot: WidgetDisplaySnapshot

    var body: some View {
        CompactCard(
            title: "WorkBuddy",
            icon: "wand.and.stars",
            value: formattedPoints,
            subtitle: statusSubtitle,
            state: state,
            unit: snapshot.workbuddyPoints == nil ? nil : settings.localized("Points"),
            valueFontSize: DashboardTypography.primaryFontSize(number: formattedPoints, compact: true)
        )
    }

    private var formattedPoints: String {
        snapshot.workbuddyPoints == nil ? settings.localized("Temporarily unavailable") : snapshot.workbuddyPointsText
    }

    private var statusSubtitle: String {
        if snapshot.workbuddyState == "unavailable" { return settings.localized("Unavailable") }
        if snapshot.workbuddyState == "stale" { return "缓存 / stale" }
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
        snapshot.workbuddyState == "live" ? .online : (snapshot.workbuddyState == "stale" ? .stale : .unavailable)
    }
}
