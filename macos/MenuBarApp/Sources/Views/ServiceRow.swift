import SwiftUI

enum CardState { case online, stale, unavailable }

struct compactCard: View {
    let title: String
    let icon: String
    let value: String
    let subtitle: String
    let state: CardState

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                Image(systemName: icon)
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
                Text(title)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundColor(.secondary)
                Spacer()
                Circle()
                    .fill(statusColor)
                    .frame(width: 5, height: 5)
            }
            Text(value)
                .font(.system(size: 20, weight: .semibold))
            Text(subtitle)
                .font(.system(size: 9))
                .foregroundColor(.secondary)
                .lineLimit(1)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10)
                .fill(Color.primary.opacity(0.04))
        )
    }

    private var statusColor: Color {
        switch state {
        case .online: return .green
        case .stale: return .orange
        case .unavailable: return .secondary
        }
    }
}

struct ServiceRow: View {
    let label: String
    let statusText: String
    let isOnline: Bool
    var toggleOn: Bool?
    var showToggle: Bool = true
    var onToggle: ((Bool) -> Void)?
    var actionLabel: String?
    var action: (() -> Void)?

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(isOnline ? Color.green : Color.secondary)
                .frame(width: 6, height: 6)

            Text(label)
                .font(.system(size: 12, weight: .medium))

            Text(statusText)
                .font(.system(size: 10))
                .foregroundColor(.secondary)

            Spacer()

            if showToggle, let onToggle = onToggle, let toggleOn = toggleOn {
                Toggle("", isOn: Binding(
                    get: { toggleOn },
                    set: { onToggle($0) }
                ))
                .toggleStyle(.switch)
                .scaleEffect(0.7)
                .frame(width: 32)
            }

            if let actionLabel = actionLabel, let action = action {
                Button(actionLabel, action: action)
                    .buttonStyle(.plain)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundColor(.accentColor)
            }
        }
        .padding(.vertical, 2)
    }
}
