import SwiftUI

struct CodexCard: View {
    let codex: CodexData

    var body: some View {
        VStack(spacing: 8) {
            // Main weekly percentage
            if let weekly = codex.weekly, let remaining = weekly.remaining {
                HStack(alignment: .lastTextBaseline) {
                    Text("Codex Weekly")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(.secondary)
                    Spacer()
                    Text(String(format: "%.0f%%", remaining))
                        .font(.system(size: 26, weight: .semibold))
                        .foregroundColor(progressColor(remaining))
                    Text("left")
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                }

                // Progress bar
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 2)
                            .fill(Color.primary.opacity(0.1))
                            .frame(height: 4)
                        RoundedRectangle(cornerRadius: 2)
                            .fill(progressColor(remaining))
                            .frame(width: geo.size.width * CGFloat(remaining / 100.0), height: 4)
                    }
                }
                .frame(height: 4)

                HStack {
                    Spacer()
                    if let fiveHour = codex.five_hour, let fiveRem = fiveHour.remaining {
                        Text("5h: \(String(format: "%.0f", fiveRem))%")
                            .font(.system(size: 10))
                            .foregroundColor(.secondary)
                    }
                }
            } else if let fiveHour = codex.five_hour, let remaining = fiveHour.remaining {
                HStack(alignment: .lastTextBaseline) {
                    Text("Codex 5h")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(.secondary)
                    Spacer()
                    Text(String(format: "%.0f%%", remaining))
                        .font(.system(size: 26, weight: .semibold))
                        .foregroundColor(progressColor(remaining))
                    Text("left")
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                }

                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 2)
                            .fill(Color.primary.opacity(0.1))
                            .frame(height: 4)
                        RoundedRectangle(cornerRadius: 2)
                            .fill(progressColor(remaining))
                            .frame(width: geo.size.width * CGFloat(remaining / 100.0), height: 4)
                    }
                }
                .frame(height: 4)

            } else {
                placeholderContent
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var placeholderContent: some View {
        HStack {
            Image(systemName: "chart.bar.fill")
                .foregroundColor(.secondary)
            Text("Codex: No data")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
    }

    private func progressColor(_ value: Double) -> Color {
        if value > 70 { return .green }
        if value >= 30 { return .yellow }
        return .red
    }
}
