import SwiftUI

struct SystemCard: View {
    @EnvironmentObject private var settings: AppSettings
    let data: SystemData

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                Text(data.label ?? "System")
                    .font(.system(size: 12, weight: .medium))
                Spacer()
                Circle()
                    .fill(statusColor)
                    .frame(width: 6, height: 6)
                Text(settings.localized(data.status ?? "Unavailable"))
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }

            if !details.isEmpty {
                Text(details)
                    .font(.system(size: DashboardTypography.status))
                    .foregroundColor(.secondary)
                    .lineLimit(2)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10)
                .fill(Color.primary.opacity(0.04))
        )
    }

    private var details: String {
        [
            data.platform,
            data.cpu.map { "CPU \($0)" },
            data.ram.map { "RAM \($0)" },
            data.gpu.map { "GPU \($0)" }
        ]
        .compactMap { value in
            guard let value, !value.isEmpty else { return nil }
            return value
        }
        .joined(separator: " · ")
    }

    private var statusColor: Color {
        switch data.status?.lowercased() {
        case "online", "healthy": return .green
        case "cached": return .orange
        default: return .secondary
        }
    }
}
