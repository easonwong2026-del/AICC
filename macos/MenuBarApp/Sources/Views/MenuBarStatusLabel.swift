import SwiftUI

struct MenuBarStatusLabel: View {
    let remaining: Double?
    let showCodexStatus: Bool
    let showBalance: Bool
    let tooltip: String?

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
        Group {
            if showCodexStatus {
                HStack(spacing: 4) {
                    Circle()
                        .fill(color)
                        .frame(width: 7, height: 7)
                    Text(showBalance ? "AI \(quotaText)" : "AI")
                        .font(.system(size: 12, weight: .medium, design: .rounded))
                        .monospacedDigit()
                }
            } else {
                Image(systemName: "chart.bar.fill")
                    .font(.system(size: 12, weight: .medium))
            }
        }
        .help(tooltip ?? (showCodexStatus ? "Codex \(quotaText)" : "AICC"))
    }
}
