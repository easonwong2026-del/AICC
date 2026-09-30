import SwiftUI

struct DeepSeekCard: View {
    @EnvironmentObject private var settings: AppSettings
    let data: DeepSeekData
    let snapshot: WidgetDisplaySnapshot

    var body: some View {
        CompactCard(
            title: "DeepSeek",
            icon: "brain.head.profile",
            value: formattedBalance,
            subtitle: settings.localized(snapshot.deepseekCompactStatusText),
            state: state,
            unit: balanceUnit,
            valueFontSize: DashboardTypography.secondaryMetric,
            valueWeight: .semibold
        )
    }

    private var formattedBalance: String {
        snapshot.deepseekState == "unavailable" ? settings.localized("Temporarily unavailable") : snapshot.deepseekBalanceText
    }

    private var balanceUnit: String? {
        snapshot.deepseekState == "unavailable" ? nil : snapshot.deepseekCurrency
    }

    private var state: CardState {
        snapshot.deepseekState == "live" ? .online : (snapshot.deepseekState == "stale" ? .stale : .unavailable)
    }
}
