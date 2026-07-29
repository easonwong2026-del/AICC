import SwiftUI

struct DashboardView: View {
    @Environment(\.openSettings) private var openSettings
    @EnvironmentObject var api: APIService
    @EnvironmentObject var ocx: OpenCodexController
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var monitor: CodexLaunchMonitor

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
            if settings.menuBarShowOpenCodex || settings.menuBarShowSystemHealth {
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

                Button(action: presentSettings) {
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

    // MARK: - Services

    private var servicesSection: some View {
        VStack(spacing: 8) {
            if settings.menuBarShowOpenCodex {
                ServiceRow(
                    label: "OpenCodex",
                    statusText: settings.localized(ocx.status.label),
                    isOnline: ocx.status.isRunning,
                    toggleOn: ocx.status.isRunning || (ocx.status == .starting),
                    onToggle: { newValue in
                        Task {
                            if newValue { await ocx.ensure() }
                            else { await ocx.stop() }
                        }
                    },
                    actionLabel: "Open Codex",
                    action: { monitor.openCodex() }
                )
            }
            if settings.menuBarShowSystemHealth {
                ServiceRow(
                    label: "System Health",
                    statusText: systemHealthText,
                    isOnline: systemIsHealthy,
                    showToggle: false,
                    actionLabel: nil,
                    action: nil
                )
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private var systemHealthText: String {
        if let health = api.health {
            return settings.localized(health.status)
        }
        guard let system = api.status?.system else { return settings.localized("Unknown") }
        return settings.localized(system.status == "Online" ? "Healthy" : system.status ?? "Unavailable")
    }

    private var systemIsHealthy: Bool {
        if let health = api.health {
            return health.status == "healthy"
        }
        return api.status?.system?.status == "Online"
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
            Menu {
                Button("About AICC") { showAbout() }
                Divider()
                Button("Quit AICC") { NSApp.terminate(nil) }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(.system(size: 13))
            }
            .buttonStyle(.plain)
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .frame(width: 24, height: 24)
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

    private func showAbout() {
        let alert = NSAlert()
        alert.messageText = "AICC"
        alert.informativeText = settings.localized("About Description")
        alert.alertStyle = .informational
        alert.runModal()
    }

    private func presentSettings() {
        NSApp.activate(ignoringOtherApps: true)
        openSettings()
        DispatchQueue.main.async {
            NSApp.activate(ignoringOtherApps: true)
        }
    }
}
