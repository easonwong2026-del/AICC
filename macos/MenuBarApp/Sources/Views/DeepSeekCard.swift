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
            subtitle: consumptionText,
            state: state,
            unit: balanceUnit,
            valueFontSize: DashboardTypography.primaryFontSize(number: formattedBalance, compact: true)
        )
    }

    private var formattedBalance: String {
        snapshot.deepseekState == "unavailable" ? settings.localized("Temporarily unavailable") : snapshot.deepseekBalanceText
    }

    private var balanceUnit: String? {
        snapshot.deepseekState == "unavailable" ? nil : snapshot.deepseekCurrency
    }

    private var consumptionText: String {
        guard snapshot.deepseekState == "live",
              let used = data.usage?.first(where: { $0.currency == snapshot.deepseekCurrency })?.used_today,
              let value = Double(used), value > 0 else { return snapshot.deepseekStatusText }
        return snapshot.deepseekStatusText + " · " + String(format: settings.localized("Today %@"),
                                                           "¥\(String(format: "%.2f", value))")
    }

    private var state: CardState {
        snapshot.deepseekState == "live" ? .online : (snapshot.deepseekState == "stale" ? .stale : .unavailable)
    }
}
