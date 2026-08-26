import SwiftUI

struct DashboardRootView: View {
    @ObservedObject var api: APIService
    @ObservedObject var ocx: OpenCodexController
    @ObservedObject var settings: AppSettings
    let openSettings: () -> Void

    var body: some View {
        ScrollView(.vertical) {
            DashboardView(openSettings: openSettings)
        }
        .frame(width: 350)
        .frame(maxHeight: 640)
        .environmentObject(api)
        .environmentObject(ocx)
        .environmentObject(settings)
        .environment(\.locale, settings.locale)
        .preferredColorScheme(settings.preferredColorScheme)
        .id(settings.presentationIdentity)
    }
}

struct DashboardView: View {
    let openSettings: () -> Void
    @EnvironmentObject var api: APIService
    @EnvironmentObject var ocx: OpenCodexController
    @EnvironmentObject var settings: AppSettings

    init(openSettings: @escaping () -> Void = {}) {
        self.openSettings = openSettings
    }

    var body: some View {
        VStack(spacing: 0) {
            headerSection
            if settings.menuBarShowCodexStatus {
                Divider().padding(.horizontal, 14)
                codexSection
            }
            if settings.menuBarShowWorkBuddy || settings.menuBarShowDeepSeek {
                Divider().padding(.horizontal, 14)
                miniCardsSection
            }
            Divider().padding(.horizontal, 14)
            systemSection
            if settings.menuBarShowOpenCodex {
                Divider().padding(.horizontal, 14)
                servicesSection
            }
            Divider().padding(.horizontal, 14)
            footerSection
        }
        .frame(width: 350)
        .background(VisualEffect.material)
    }

    // MARK: - Header

    private var headerSection: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("AICC")
                    .font(.system(size: 18, weight: .bold))
                statusSummaryText
                    .font(.system(size: 10))
            }
            Spacer()
            HStack(spacing: 12) {
                Button(action: { Task { await api.fetchStatus(force: true) } }) {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 13, weight: .medium))
                }
                .buttonStyle(.plain)
                .help("Refresh")

                Button(action: openSettings) {
                    Image(systemName: "gearshape")
                        .font(.system(size: 13, weight: .medium))
                }
                .buttonStyle(.plain)
                .help("Settings")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private var statusSummaryText: some View {
        let s = api.state
        switch s {
        case .ready where api.allServicesOk: return Text("All services online").foregroundColor(.green)
        case .ready: return Text("Some services unavailable").foregroundColor(.orange)
        case .stale: return Text("Using cached data").foregroundColor(.secondary)
        case .unavailable: return Text("Server offline").foregroundColor(.red)
        case .error(let m): return Text(m).foregroundColor(.red)
        case .loading: return Text("Checking...").foregroundColor(.secondary)
        }
    }

    // MARK: - Codex

    private var codexSection: some View {
        VStack(spacing: 8) {
            if let codex = api.status?.codex {
                CodexCard(codex: codex)
            } else {
                placeholderCard(title: "Codex", icon: "chart.bar.fill")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    // MARK: - Mini Cards

    private var miniCardsSection: some View {
        HStack(spacing: 10) {
            if settings.menuBarShowWorkBuddy {
                if let wb = api.status?.workbuddy {
                    WorkBuddyCard(data: wb)
                } else {
                    placeholderCard(title: "WorkBuddy", icon: "wand.and.stars")
                }
            }
            if settings.menuBarShowDeepSeek {
                if let ds = api.status?.deepseek {
                    DeepSeekCard(data: ds)
                } else {
                    placeholderCard(title: "DeepSeek", icon: "brain.head.profile")
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    // MARK: - System

    private var systemSection: some View {
        VStack(spacing: 8) {
            if let system = api.status?.system {
                SystemCard(data: system)
            } else {
                placeholderCard(title: "System", icon: "desktopcomputer")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    // MARK: - Services

    private var servicesSection: some View {
        VStack(spacing: 8) {
            if settings.menuBarShowOpenCodex {
                ServiceRow(
                    label: "OpenCodex",
                    statusText: settings.localized(ocx.status.label),
                    isOnline: ocx.status.isRunning,
                    toggleOn: ocx.status.isToggleOn,
                    isBusy: ocx.status.isBusy,
                    statusColor: ocxStatusColor,
                    onToggle: { newValue in
                        Task {
                            if newValue { await ocx.ensure() }
                            else { await ocx.stop() }
                        }
                    },
                    actionLabel: nil,
                    action: nil
                )
                DashboardActionRow(
                    label: "OpenCodex Dashboard",
                    actionLabel: "Open Dashboard",
                    isEnabled: ocx.dashboardURL != nil,
                    action: { _ = ocx.openDashboard() }
                )
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private var ocxStatusColor: Color {
        switch ocx.status {
        case .running:
            return .green
        case .unhealthy:
            return .red
        case .checking, .starting, .stopping:
            return .yellow
        case .unknown, .notInstalled, .stopped:
            return .secondary
        }
    }

    // MARK: - Footer

    private var footerSection: some View {
        HStack {
            if let last = api.lastRefresh {
                Text("Updated \(last, style: .relative) ago")
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }
            Spacer()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
    }

    // MARK: - Helpers

    private func placeholderCard(title: String, icon: String) -> some View {
        VStack(spacing: 4) {
            Image(systemName: icon)
                .font(.system(size: 16))
                .foregroundColor(.secondary)
            Text(settings.localized(title) + "\n" + settings.localized("No data"))
                .font(.system(size: 10))
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 14)
        .background(
            RoundedRectangle(cornerRadius: 10)
                .fill(Color.primary.opacity(0.04))
        )
    }

}
