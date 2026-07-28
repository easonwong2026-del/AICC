import SwiftUI

struct DeepSeekCard: View {
    let data: DeepSeekData

    var body: some View {
        CompactCard(
            title: "DeepSeek",
            icon: "brain.head.profile",
            value: formattedBalance,
            subtitle: consumptionText,
            state: state
        )
    }

    private var formattedBalance: String {
        guard let balances = data.balances, !balances.isEmpty else { return "--" }
        let cny = balances.first(where: { $0.currency == "CNY" }) ?? balances[0]
        guard let total = Double(cny.total_balance ?? "") else { return "--" }
        return String(format: "¥%.2f", total)
    }

    private var consumptionText: String {
        guard let usage = data.usage, !usage.isEmpty else {
            return data.status == "Online" ? "Online" : (data.status ?? "--")
        }
        let cnyUsed = usage.first(where: { $0.currency == "CNY" })?.used_today
        if let used = cnyUsed, let value = Double(used), value > 0 {
            return "Today ¥\(String(format: "%.2f", value))"
        }
        return "Online"
    }

    private var state: CardState {
        if data.status == "Online" { return .online }
        if data.status == "Not configured" { return .unavailable }
        if data.balances?.isEmpty == false { return .stale }
        return .unavailable
    }
}
