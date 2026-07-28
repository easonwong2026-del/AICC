import SwiftUI

struct MenuBarStatusLabel: View {
    let status: StatusResponse?

    private var remaining: Double? {
        status?.codex?.weekly?.remaining ?? status?.codex?.five_hour?.remaining
    }

    private var color: Color {
        guard let remaining else { return .secondary }
        if remaining > 70 { return .green }
        if remaining >= 30 { return .yellow }
        return .red
    }

    private var quotaText: String {
        guard let remaining else { return "--" }
        return String(format: "%.0f%%", max(0, min(100, remaining)))
    }

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(color)
                .frame(width: 7, height: 7)
            Text("AI \(quotaText)")
                .font(.system(size: 12, weight: .medium, design: .rounded))
                .monospacedDigit()
        }
        .help("Codex \(quotaText)")
    }
}
