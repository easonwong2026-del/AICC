import SwiftUI

struct WorkBuddyCard: View {
    let data: WorkBuddyData

    var body: some View {
        CompactCard(
            title: "WorkBuddy",
            icon: "wand.and.stars",
            value: formattedPoints,
            subtitle: statusSubtitle,
            state: state
        )
    }

    private var formattedPoints: String {
        guard let points = data.points else { return "--" }
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = true
        formatter.maximumFractionDigits = 2
        formatter.minimumFractionDigits = 0
        return formatter.string(from: NSNumber(value: points)) ?? String(points)
    }

    private var statusSubtitle: String {
        let status = data.balance_state ?? (data.points == nil ? "Unavailable" : "Connected")
        if data.points == nil {
            if data.balance_error_code == "bridge_unavailable" {
                return "未连接 · 设置 → 重连 WorkBuddy"
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
        if seconds < 60 { return "\(seconds)s ago" }
        if seconds < 3600 { return "\(seconds / 60)m ago" }
        if seconds < 86400 { return "\(seconds / 3600)h ago" }
        return "\(seconds / 86400)d ago"
    }

    private var state: CardState {
        if data.balance_stale == true || data.balance_state == "Cached" { return .stale }
        if data.balance_state == "Connected" { return .online }
        if data.points == nil { return .unavailable }
        return .online
    }
}
