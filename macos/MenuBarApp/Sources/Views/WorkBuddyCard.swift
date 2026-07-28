import SwiftUI

struct WorkBuddyCard: View {
    let data: WorkBuddyData

    var body: some View {
        CompactCard(
            title: "WorkBuddy",
            icon: "wand.and.stars",
            value: formattedPoints,
            subtitle: data.reset_text ?? "",
            state: state
        )
    }

    private var formattedPoints: String {
        guard let points = data.points else { return "--" }
        if points >= 1000 {
            return String(format: "%.1fk", points / 1000)
        }
        return String(format: "%.0f", points)
    }

    private var state: CardState {
        if data.balance_state == "Connected" { return .online }
        if data.balance_state == "Cached" { return .stale }
        if data.points == nil { return .unavailable }
        return .online
    }
}
