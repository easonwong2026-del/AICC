import SwiftUI
import AppKit
import UniformTypeIdentifiers

struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var ocx: OpenCodexController
    @EnvironmentObject private var api: APIService
    @EnvironmentObject private var server: ServerManager
    @EnvironmentObject private var loginAtLaunch: LaunchAtLoginService

    var body: some View {
        TabView {
            GeneralSettingsView()
                .tabItem { Label("General", systemImage: "gearshape") }
            AIProvidersSettingsView()
                .tabItem { Label("AI Providers", systemImage: "cpu") }
            DevicesSettingsView()
                .tabItem { Label("Devices", systemImage: "display.2") }
            MenuBarSettingsView()
                .tabItem { Label("Menu Bar", systemImage: "menubar.rectangle") }
            AdvancedSettingsView()
                .tabItem { Label("Advanced", systemImage: "wrench.and.screwdriver") }
        }
        .frame(width: 640, height: 480)
        .onAppear {
            loginAtLaunch.refresh()
            settings.launchAtLogin = loginAtLaunch.isEnabled
        }
    }
}

// MARK: - General

private struct GeneralSettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var api: APIService
    @EnvironmentObject private var loginAtLaunch: LaunchAtLoginService

    var body: some View {
        Form {
            Section("Startup") {
                Toggle("Launch AICC at login", isOn: Binding(
                    get: { loginAtLaunch.isEnabled },
                    set: { enabled in
                        settings.launchAtLogin = enabled
                        loginAtLaunch.setEnabled(enabled)
                    }
                ))
                if let error = loginAtLaunch.errorMessage {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            }

            Section("Refresh") {
                Picker("Automatic refresh", selection: $settings.autoRefreshInterval) {
                    Text("60 seconds").tag(60.0)
                    Text("120 seconds").tag(120.0)
                    Text("300 seconds").tag(300.0)
                }
                Text("Menu bar updates use the same cached status and do not trigger an additional provider request.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("Appearance") {
                Picker("Language", selection: $settings.languageCode) {
                    ForEach(AppLanguage.allCases) { language in
                        Text(LocalizedStringKey(language.displayNameKey)).tag(language.rawValue)
                    }
                }
                Picker("Theme", selection: $settings.themeMode) {
                    Text("System").tag("system")
                    Text("Light").tag("light")
                    Text("Dark").tag("dark")
                }
            }
        }
        .formStyle(.grouped)
        .onChange(of: settings.autoRefreshInterval) { _, interval in
            api.startAutoRefresh(interval: interval)
        }
    }
}

// MARK: - AI Providers

private struct AIProvidersSettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var api: APIService

    var body: some View {
        Form {
            Section("Providers") {
                if let providers = api.providers?.providers, !providers.isEmpty {
                    ForEach(sortedProviders) { provider in
                        DynamicProviderRow(provider: provider)
                    }
                    Button("Reset default order") {
                        settings.resetProviderOrder()
                    }
                } else if case .ready = api.state {
                    // Older server without /api/providers: keep the legacy rows.
                    DataSourceRow(
                        name: "OpenAI",
                        status: openAIStatus,
                        lastUpdate: "Account used by Codex",
                        action: refresh
                    )
                    DataSourceRow(
                        name: "Codex",
                        status: codexStatus,
                        lastUpdate: updateText(api.status?.collection?.codex),
                        action: refresh
                    )
                    DataSourceRow(
                        name: "WorkBuddy",
                        status: workBuddyStatus,
                        lastUpdate: workBuddyLastUpdate,
                        action: refresh
                    )
                    DataSourceRow(
                        name: "DeepSeek",
                        status: deepSeekStatus,
                        lastUpdate: updateText(api.status?.collection?.deepseek),
                        action: refresh
                    )
                } else {
                    Text("Loading providers…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            Section {
                Text("Providers are listed dynamically from the local manifest. Use the arrows to reorder, the switch to show or hide a provider, and the info button for diagnostics. Credentials are read from the existing local application, environment, or Keychain integrations; AICC never displays tokens.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear {
            if api.providers == nil {
                Task { await api.fetchProviders() }
            }
        }
    }

    private var sortedProviders: [ProviderSummary] {
        ProviderPreferences.ordered(
            api.providers?.providers ?? [],
            order: settings.providerOrder,
            id: { $0.id },
            manifestSortOrder: { $0.sortOrder }
        )
    }

    private var openAIStatus: String {
        settings.localized(api.status?.codex?.available == true ? "Connected" : "Unavailable")
    }

    private var codexStatus: String {
        guard let codex = api.status?.codex else { return settings.localized("Unavailable") }
        if codex.stale == true { return settings.localized("Cached") }
        if codex.available == true { return settings.localized("Connected") }
        return settings.localized(codex.state ?? "Unavailable")
    }

    private var workBuddyStatus: String {
        guard let workbuddy = api.status?.workbuddy else { return settings.localized("Unavailable") }
        if workbuddy.balance_stale == true || workbuddy.balance_state == "Cached" {
            return settings.localized("Cached")
        }
        return settings.localized(workbuddy.points == nil ? "Unavailable" : "Connected")
    }

    private var workBuddyLastUpdate: String {
        if let error = api.status?.workbuddy?.balance_error {
            return "\(settings.localized("Error")): \(error)"
        }
        return updateText(api.status?.collection?.workbuddy)
    }

    private var deepSeekStatus: String {
        settings.localized(api.status?.deepseek?.status ?? "Unavailable")
    }

    private func updateText(_ item: CollectorStatus?) -> String {
        guard let item else { return settings.localized("No successful read") }
        if let age = item.age_seconds {
            return String(format: settings.localized("Updated %ds ago"), age)
        }
        return item.last_success ?? settings.localized("Waiting for first read")
    }

    private func refresh() {
        Task { await api.fetchStatus(force: true) }
    }
}

// MARK: - Dynamic provider row

private struct DynamicProviderRow: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var api: APIService

    let provider: ProviderSummary

    @State private var diagnosticsText: String?
    @State private var showingDiagnostics = false

    var body: some View {
        HStack(spacing: 10) {
            VStack(spacing: 2) {
                Button {
                    settings.moveProvider(provider.id, direction: -1)
                } label: {
                    Image(systemName: "chevron.up")
                        .font(.system(size: 9, weight: .semibold))
                }
                .buttonStyle(.borderless)
                .disabled(isFirst)
                Button {
                    settings.moveProvider(provider.id, direction: 1)
                } label: {
                    Image(systemName: "chevron.down")
                        .font(.system(size: 9, weight: .semibold))
                }
                .buttonStyle(.borderless)
                .disabled(isLast)
            }
            .frame(width: 16)

            Circle()
                .fill(statusColor)
                .frame(width: 7, height: 7)

            VStack(alignment: .leading, spacing: 2) {
                Text(provider.displayName)
                    .font(.system(size: 13, weight: .medium))
                    .lineLimit(1)
                Text(statusText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer()

            Toggle("", isOn: visibilityBinding)
                .labelsHidden()
                .toggleStyle(.switch)
                .scaleEffect(0.8)
                .help(settings.isProviderHidden(provider.id)
                      ? settings.localized("Show")
                      : settings.localized("Hide"))

            Button {
                Task { await api.refreshProvider(id: provider.id) }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help(settings.localized("Refresh"))
            .disabled(!provider.capabilities.contains("refresh"))

            Menu {
                Button(settings.localized("Diagnostics")) {
                    Task {
                        let result = await api.performProviderAction(
                            providerId: provider.id,
                            kind: "diagnostics"
                        )
                        diagnosticsText = result
                        showingDiagnostics = result != nil
                    }
                }
            } label: {
                Image(systemName: "info.circle")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .frame(width: 22, height: 22)
            .help(settings.localized("Diagnostics"))
            .disabled(!provider.capabilities.contains("diagnostics"))
        }
        .sheet(isPresented: $showingDiagnostics) {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text("\(provider.displayName) — Diagnostics")
                        .font(.headline)
                    Spacer()
                    Button(settings.localized("Close")) { showingDiagnostics = false }
                        .buttonStyle(.borderless)
                }
                ScrollView {
                    Text(diagnosticsText ?? settings.localized("No data"))
                        .font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(16)
            .frame(width: 460, height: 340)
        }
    }

    private var isFirst: Bool {
        settings.providerOrder.first == provider.id
    }

    private var isLast: Bool {
        settings.providerOrder.last == provider.id
    }

    private var visibilityBinding: Binding<Bool> {
        Binding(
            get: { !settings.isProviderHidden(provider.id) },
            set: { visible in settings.setProviderHidden(provider.id, hidden: !visible) }
        )
    }

    private var statusText: String {
        let stateLabel: String
        switch provider.state {
        case "connected": stateLabel = settings.localized("Connected")
        case "cached": stateLabel = settings.localized("Cached")
        case "unavailable": stateLabel = settings.localized("Unavailable")
        case "error": stateLabel = settings.localized("Error")
        case "pending": stateLabel = settings.localized("Pending")
        case "disabled": stateLabel = settings.localized("Disabled")
        default: stateLabel = provider.state
        }
        if let updated = provider.updatedAt {
            return "\(stateLabel) · \(updated)"
        }
        return stateLabel
    }

    private var statusColor: Color {
        switch provider.state {
        case "connected": return .green
        case "cached": return .orange
        case "error": return .red
        case "pending": return .yellow
        default: return .secondary
        }
    }
}

private struct DataSourceRow: View {
    let name: String
    let status: String
    let lastUpdate: String
    let action: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Circle()
                .fill(statusColor)
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 2) {
                Text(name)
                    .font(.system(size: 13, weight: .medium))
                Text(lastUpdate)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text(status)
                .font(.caption)
                .foregroundStyle(.secondary)
            Button("Refresh", action: action)
                .buttonStyle(.borderless)
        }
    }

    private var statusColor: Color {
        switch status.lowercased() {
        case "connected", "online", "healthy": return .green
        case "cached": return .orange
        default: return .secondary
        }
    }
}

// MARK: - Devices

private struct DevicesSettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var api: APIService

    var body: some View {
        Form {
            Section("Poke4S") {
                DeviceStatusRow(
                    name: "Poke4S",
                    detail: "UDP discovery 8766 · HTTP dashboard 8765",
                    status: serverStatus
                )
            }

            Section("Android Dashboard") {
                DeviceStatusRow(
                    name: "Android Dashboard",
                    detail: "Uses the same cached /api/status contract",
                    status: serverStatus
                )
            }

            Section("Sync") {
                Button("Sync Now") {
                    Task { await api.fetchStatus(force: true) }
                }
                if let updated = api.lastRefresh {
                    LabeledContent("Last sync", value: updated.formatted(date: .abbreviated, time: .standard))
                }
            }
        }
        .formStyle(.grouped)
    }

    private var serverStatus: String {
        switch api.state {
        case .ready: return settings.localized("Available")
        case .loading: return settings.localized("Checking")
        case .unavailable: return settings.localized("Offline")
        case .stale: return settings.localized("Cached")
        case .error: return settings.localized("Error")
        }
    }
}

private struct DeviceStatusRow: View {
    let name: String
    let detail: String
    let status: String

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(name)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text("Server: \(status)")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

// MARK: - Menu Bar

private struct MenuBarSettingsView: View {
    @EnvironmentObject private var settings: AppSettings

    var body: some View {
        Form {
            Section("Menu bar status") {
                Toggle("Show Codex status", isOn: $settings.menuBarShowCodexStatus)
                Toggle("Show Codex balance", isOn: $settings.menuBarShowCodexBalance)
            }

            Section("Menu bar items") {
                Toggle("WorkBuddy", isOn: $settings.menuBarShowWorkBuddy)
                Toggle("DeepSeek", isOn: $settings.menuBarShowDeepSeek)
                Toggle("OpenCodex", isOn: $settings.menuBarShowOpenCodex)
            }

            Section {
                Text("The menu bar label only shows Codex. The selected items appear after clicking it.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Advanced

private struct AdvancedSettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var ocx: OpenCodexController
    @EnvironmentObject private var api: APIService
    @EnvironmentObject private var server: ServerManager

    @State private var notice = ""
    @State private var showingClearCacheConfirmation = false

    var body: some View {
        Form {
            Section("OpenCodex") {
                HStack {
                    Text("Executable")
                    Spacer()
                    Text(ocx.detectedPath ?? settings.localized("Not detected"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Button("Redetect") {
                        Task { await ocx.detectExecutable() }
                    }
                    .buttonStyle(.borderless)
                }

                if let version = ocx.ocxVersion {
                    LabeledContent("Version", value: version)
                }
                if let port = ocx.knownPort {
                    LabeledContent("Port", value: String(port))
                }
            }

            Section("Diagnostics") {
                Toggle("Debug mode", isOn: $settings.debugMode)
                Text("Debug logging applies after the internal data service is restarted.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button("View Logs") { openDirectory(AICCPaths.logsDirectory) }
                Button("Show Data Directory") {
                    if let directory = server.dataDirectoryURL {
                        openDirectory(directory)
                    }
                }
                Button("Export Diagnostic Info") { exportDiagnostics() }
            }

            Section("Service") {
                Button("Restart Data Service") {
                    Task {
                        let success = await server.restartServer()
                        notice = settings.localized(
                            success ? "Data service restarted." : "Unable to restart data service."
                        )
                        if success { await api.fetchStatus() }
                    }
                }
                Button("Clear Cache", role: .destructive) {
                    showingClearCacheConfirmation = true
                }
                if !notice.isEmpty {
                    Text(notice)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .confirmationDialog(
            "Clear AICC cached data?",
            isPresented: $showingClearCacheConfirmation,
            titleVisibility: .visible
        ) {
            Button("Clear Cache", role: .destructive) { clearCache() }
            Button("Cancel", role: .cancel) { }
        } message: {
            Text("Provider credentials are not stored in this cache.")
        }
    }

    private func openDirectory(_ directory: URL) {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        NSWorkspace.shared.open(directory)
    }

    private func clearCache() {
        guard let directory = server.dataDirectoryURL else {
            notice = settings.localized("Data directory is unavailable.")
            return
        }
        do {
            let count = try CacheManager.clear(in: directory)
            notice = count == 0
                ? settings.localized("No cached files found.")
                : String(format: settings.localized("Cleared %d cached file(s)."), count)
            Task { await api.fetchStatus(force: true) }
        } catch {
            notice = settings.localized("Cache cleanup failed:") + " \(error.localizedDescription)"
        }
    }

    private func exportDiagnostics() {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.json]
        panel.nameFieldStringValue = "AICC-Diagnostics.json"
        guard panel.runModal() == .OK, let url = panel.url else { return }

        let payload = DiagnosticsPayload(
            version: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev",
            generatedAt: ISO8601DateFormatter().string(from: Date()),
            status: api.status
        )
        do {
            let data = try JSONEncoder().encode(payload)
            try data.write(to: url, options: .atomic)
            notice = settings.localized("Diagnostic info exported.")
        } catch {
            notice = settings.localized("Export failed:") + " \(error.localizedDescription)"
        }
    }
}

private struct DiagnosticsPayload: Codable {
    let version: String
    let generatedAt: String
    let status: StatusResponse?
}
