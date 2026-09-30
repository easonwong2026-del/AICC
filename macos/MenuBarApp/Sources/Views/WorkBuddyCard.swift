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
            valueFontSize: DashboardTypography.secondaryMetric,
            valueWeight: .semibold
        )
    }

    private var formattedPoints: String {
        snapshot.workbuddyPoints == nil ? settings.localized("Temporarily unavailable") : snapshot.workbuddyPointsText
    }

    private var statusSubtitle: String {
        if snapshot.workbuddyState != "unavailable" {
            return settings.localized(snapshot.workbuddyCompactStatusText)
        }
        if data.balance_error_code == "bridge_unavailable" {
            return settings.localized("Not connected · Settings → reconnect WorkBuddy")
        }
        return settings.localized(data.balance_error ?? data.balance_state ?? "Unavailable")
    }

    private var state: CardState {
        snapshot.workbuddyState == "live" ? .online : (snapshot.workbuddyState == "stale" ? .stale : .unavailable)
    }
}
