import SwiftUI

// MARK: - Dynamic provider card

/// Generic card driven entirely by a manifest `ProviderSummary`. Adding a new
/// built-in provider never requires a new SwiftUI card.
struct ProviderCard: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var api: APIService

    let provider: ProviderSummary

    @State private var diagnosticsText: String?
    @State private var showingDiagnostics = false
    @State private var actionNotice: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            primaryMetricArea
            secondaryMetricRows
            footer
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.primary.opacity(0.04))
        )
        .sheet(isPresented: $showingDiagnostics) {
            diagnosticsSheet
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 6) {
            Image(systemName: provider.icon ?? "circle.grid.2x2")
                .font(.system(size: 12))
                .foregroundColor(.secondary)
            Text(provider.displayName)
                .font(.system(size: 13, weight: .semibold))
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer()
            ProviderStateBadge(provider: provider, localized: settings.localized)
        }
    }

    // MARK: Primary metric

    @ViewBuilder
    private var primaryMetricArea: some View {
        let primaries = provider.primaryMetrics
        if let first = primaries.first {
            ProviderPrimaryMetricView(metric: first, settings: settings)
        } else if let fallback = provider.metrics.first {
            // No primary metric: degrade to the first available metric.
            ProviderPrimaryMetricView(metric: fallback, settings: settings)
        } else {
            Text(settings.localized("No data"))
                .font(.system(size: 20, weight: .semibold))
                .foregroundColor(.secondary)
        }

        if primaries.count >= 2 {
            ProviderSecondaryMetricView(metric: primaries[1], settings: settings, emphasized: true)
        }
    }

    // MARK: Secondary metrics

    @ViewBuilder
    private var secondaryMetricRows: some View {
        let rows = provider.secondaryMetrics.prefix(3)
        if !rows.isEmpty {
            VStack(spacing: 4) {
                ForEach(rows) { metric in
                    ProviderSecondaryMetricView(metric: metric, settings: settings, emphasized: false)
                }
            }
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: 6) {
            if let updated = provider.updatedAt {
                Text(settings.localized("Updated") + " \(updated)")
                    .font(.system(size: DashboardTypography.timestamp))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            if let notice = actionNotice {
                Text(notice)
                    .font(.system(size: DashboardTypography.timestamp))
                    .foregroundColor(.orange)
                    .lineLimit(1)
            }
            Spacer()
            ProviderActionMenu(
                provider: provider,
                onRefresh: {
                    Task {
                        actionNotice = nil
                        await api.refreshProvider(id: provider.id)
                    }
                },
                onAction: { action in
                    Task {
                        actionNotice = nil
                        if action.kind == "diagnostics" {
                            let result = await api.performProviderAction(
                                providerId: provider.id,
                                kind: action.kind
                            )
                            diagnosticsText = result
                            showingDiagnostics = result != nil
                        } else {
                            _ = await api.performProviderAction(
                                providerId: provider.id,
                                kind: action.kind
                            )
                        }
                    }
                }
            )
        }
    }

    private var diagnosticsSheet: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("\(provider.displayName) — Diagnostics")
                    .font(.headline)
                Spacer()
                Button("Close") { showingDiagnostics = false }
                    .buttonStyle(.borderless)
            }
            ScrollView {
                Text(diagnosticsText ?? "No diagnostics")
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(16)
        .frame(width: 460, height: 340)
    }
}

// MARK: - Primary metric (large number + unit)

struct ProviderPrimaryMetricView: View {
    let metric: ProviderMetric
    let settings: AppSettings

    var body: some View {
        let display = MetricFormatter.format(
            value: metric.value,
            valueType: metric.safeValueType,
            format: metric.safeFormat,
            unit: metric.unit
        )
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                .foregroundColor(.secondary)
                .lineLimit(1)
            if display.placeholder {
                Text(settings.localized("Temporarily unavailable"))
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundColor(.secondary)
            } else if let number = display.number {
                HStack(alignment: .lastTextBaseline, spacing: 4) {
                    Text(number)
                        .font(.system(
                            size: DashboardTypography.primaryFontSize(number: number),
                            weight: .bold
                        ))
                        .lineLimit(1)
                        .minimumScaleFactor(0.75)
                        .allowsTightening(true)
                    if !display.unit.isEmpty {
                        Text(display.unit)
                            .font(.system(size: DashboardTypography.unit, weight: .medium))
                            .foregroundColor(.secondary)
                    }
                }
            } else {
                Text(settings.localized("Temporarily unavailable"))
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundColor(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var label: String {
        if settings.language == .english, let english = MetricEnglishLabels.lookup(metric.key) {
            return english
        }
        return metric.label
    }
}

// MARK: - Secondary metric row

struct ProviderSecondaryMetricView: View {
    let metric: ProviderMetric
    let settings: AppSettings
    var emphasized = false

    var body: some View {
        let display = MetricFormatter.format(
            value: metric.value,
            valueType: metric.safeValueType,
            format: metric.safeFormat,
            unit: metric.unit
        )
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(label)
                .font(.system(size: DashboardTypography.metricLabel))
                .foregroundColor(.secondary)
                .lineLimit(1)
            Spacer()
            if display.placeholder {
                Text("--")
                    .font(.system(size: emphasized ? DashboardTypography.secondaryMetric : 13, weight: .medium))
                    .foregroundColor(.secondary)
            } else {
                HStack(alignment: .lastTextBaseline, spacing: 3) {
                    Text(display.number ?? "")
                        .font(.system(
                            size: emphasized ? DashboardTypography.secondaryMetric : 13,
                            weight: emphasized ? .semibold : .medium
                        ))
                        .lineLimit(1)
                    if !display.unit.isEmpty {
                        Text(display.unit)
                            .font(.system(size: 10))
                            .foregroundColor(.secondary)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var label: String {
        if settings.language == .english, let english = MetricEnglishLabels.lookup(metric.key) {
            return english
        }
        return metric.label
    }
}

// MARK: - State badge

struct ProviderStateBadge: View {
    let provider: ProviderSummary
    let localized: (String) -> String

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(statusColor)
                .frame(width: 6, height: 6)
            Text(label)
                .font(.system(size: DashboardTypography.status, weight: .medium))
                .foregroundColor(statusColor)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(Capsule().fill(statusColor.opacity(0.12)))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(accessibilityLabel)
    }

    private var statusColor: Color {
        switch provider.state {
        case "connected": return .green
        case "cached": return .orange
        case "unavailable": return .secondary
        case "error": return .red
        case "pending": return .yellow
        case "disabled": return .secondary
        default: return .secondary
        }
    }

    private var label: String {
        switch provider.state {
        case "connected": return localized("Connected")
        case "cached": return localized("Cached")
        case "unavailable": return localized("Unavailable")
        case "error": return localized("Error")
        case "pending": return localized("Pending")
        case "disabled": return localized("Disabled")
        default: return provider.state
        }
    }

    private var accessibilityLabel: String {
        "\(provider.displayName), \(label)"
    }
}

// MARK: - Action menu

struct ProviderActionMenu: View {
    let provider: ProviderSummary
    let onRefresh: () -> Void
    let onAction: (ProviderAction) -> Void

    var body: some View {
        Menu {
            if provider.capabilities.contains("refresh") {
                Button("Refresh", systemImage: "arrow.clockwise", action: onRefresh)
            }
            ForEach(provider.actions.filter { $0.kind != "refresh" }) { action in
                Button(action.label, systemImage: icon(for: action.kind)) {
                    onAction(action)
                }
            }
        } label: {
            Image(systemName: "ellipsis.circle")
                .font(.system(size: 13))
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .frame(width: 22, height: 22)
        .help(provider.displayName)
    }

    private func icon(for kind: String) -> String {
        switch kind {
        case "reconnect": return "arrow.triangle.2.circlepath"
        case "diagnostics": return "stethoscope"
        default: return "gearshape"
        }
    }
}

// MARK: - English label fallback for known metric keys

enum MetricEnglishLabels {
    static let mapping: [String: String] = [
        "weekly_remaining": "Codex Weekly Left",
        "weekly_reset": "Weekly Reset",
        "five_hour_remaining": "5 Hour Left",
        "five_hour_reset": "5 Hour Reset",
        "points": "Points Left",
        "used_today": "Used Today",
        "total_points": "Total Points",
        "cache_age": "Cache Age",
        "balance": "Balance",
        "status": "Status",
    ]

    static func lookup(_ key: String) -> String? {
        mapping[key]
    }
}
