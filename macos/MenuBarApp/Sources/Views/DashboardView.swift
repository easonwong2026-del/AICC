import SwiftUI

struct DashboardView: View {
    @Environment(\.openSettings) private var openSettings
    @EnvironmentObject var api: APIService
    @EnvironmentObject var ocx: OpenCodexController
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        VStack(spacing: 0) {
            headerSection
            if settings.menuBarShowCodexStatus && !settings.isProviderHidden("codex") {
                Divider().padding(.horizontal, 14)
                codexSection
            }
            if (settings.menuBarShowWorkBuddy && !settings.isProviderHidden("workbuddy"))
                || (settings.menuBarShowDeepSeek && !settings.isProviderHidden("deepseek")) {
                Divider().padding(.horizontal, 14)
                miniCardsSection
            }
            dynamicProviderSection
            if settings.menuBarShowOpenCodex {
                Divider().padding(.horizontal, 14)
                servicesSection
            }
            Divider().padding(.horizontal, 14)
            footerSection
        }
        .frame(width: 350)
        .background(VisualEffect.material)
        .onAppear { ocx.panelDidAppear() }
        .onDisappear { ocx.panelDidDisappear() }
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
            if settings.menuBarShowWorkBuddy && !settings.isProviderHidden("workbuddy") {
                if let wb = api.status?.workbuddy {
                    WorkBuddyCard(data: wb)
                } else {
                    placeholderCard(title: "WorkBuddy", icon: "wand.and.stars")
                }
            }
            if settings.menuBarShowDeepSeek && !settings.isProviderHidden("deepseek") {
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

    // MARK: - Dynamic Providers

    /// Manifest-driven cards. Legacy cards (codex, workbuddy, deepseek) keep
    /// rendering through their existing components during the migration
    /// period; every other provider — including future built-ins and the dev
    /// example provider — renders here without any dedicated SwiftUI card.
    @ViewBuilder
    private var dynamicProviderSection: some View {
        let legacy = Set(["codex", "workbuddy", "deepseek"])
        let providers = api.providers?.providers ?? []
        let visible = ProviderPreferences.ordered(
            providers.filter { !legacy.contains($0.id) && !settings.isProviderHidden($0.id) },
            order: settings.providerOrder,
            id: { $0.id },
            manifestSortOrder: { $0.sortOrder }
        )
        if !visible.isEmpty {
            Divider().padding(.horizontal, 14)
            VStack(spacing: 8) {
                ForEach(visible) { provider in
                    ProviderCard(provider: provider)
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
        }
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
        let shortVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"
        let buildVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
        let description = settings.localized("About Description")
        let versionLabel = settings.localized("Version")
        alert.informativeText = "\(description)\n\n\(versionLabel) \(shortVersion) (\(buildVersion))"
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
