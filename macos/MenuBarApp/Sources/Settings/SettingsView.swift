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
                        Text(language.pickerDisplayName(localize: settings.localized)).tag(language.rawValue)
                    }
                }
                Picker("Theme", selection: $settings.themeMode) {
                    Text("System").tag("system")
                    Text("Light").tag("light")
                    Text("Dark").tag("dark")
                }
            }

            AboutAndUpdatesSettingsView()
        }
        .formStyle(.grouped)
        .onChange(of: settings.autoRefreshInterval) { _, interval in
            api.startAutoRefresh(interval: interval)
        }
    }
}

private struct AboutAndUpdatesSettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @StateObject private var updateService = UpdateService()

    private var versionInfo: AppVersionInfo {
        AppVersionProvider().current
    }

    private var isChecking: Bool {
        updateService.state == .checking
    }

    var body: some View {
        Section {
            Text("AICC")
                .font(.headline)
            LabeledContent(settings.localized("Version"), value: versionInfo.shortVersion)
            LabeledContent(settings.localized("Build"), value: versionInfo.buildVersion)

            HStack(spacing: 10) {
                Button {
                    Task { await updateService.checkForUpdates() }
                } label: {
                    HStack(spacing: 6) {
                        if isChecking {
                            ProgressView()
                                .controlSize(.small)
                        }
                        Text(settings.localized("Check for Updates"))
                    }
                }
                .disabled(isChecking)

                Spacer(minLength: 0)
            }

            updateStatus
        } header: {
            Text(settings.localized("About & Updates"))
        }
    }

    @ViewBuilder
    private var updateStatus: some View {
        switch updateService.state {
        case .idle:
            Text(settings.localized("Updates are checked only when you click the button."))
                .font(.caption)
                .foregroundStyle(.secondary)
        case .checking:
            Text(settings.localized("Checking for Updates…"))
                .font(.caption)
                .foregroundStyle(.secondary)
        case .upToDate:
            Text(settings.localized("You're up to date"))
                .font(.caption)
                .foregroundStyle(.secondary)
        case .updateAvailable(let info):
            VStack(alignment: .leading, spacing: 6) {
                Text(String(format: settings.localized("Update available: %@"), info.version))
                    .font(.caption)
                Button(settings.localized("View Update")) {
                    openUpdate(info)
                }
                .buttonStyle(.link)
            }
        case .failed:
            Text(settings.localized("Update check failed"))
                .font(.caption)
                .foregroundStyle(.secondary)
        case .notConfigured:
            VStack(alignment: .leading, spacing: 6) {
                Text(settings.localized("Update source not configured"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button(settings.localized("Open Release Page")) {
                    openHTTPS(updateService.releasePageURL)
                }
                .buttonStyle(.link)
            }
        }
    }

    private func openUpdate(_ info: UpdateInfo) {
        openHTTPS(info.downloadURL ?? info.releaseNotesURL ?? updateService.releasePageURL)
    }

    private func openHTTPS(_ url: URL) {
        guard UpdateManifestConfiguration.httpsURL(url.absoluteString) != nil else { return }
        NSWorkspace.shared.open(url)
    }
}

// MARK: - AI Providers

private struct AIProvidersSettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var api: APIService

    var body: some View {
        Form {
            Section("Providers") {
                if api.status != nil {
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
                    Button("Reconnect WorkBuddy") {
                        Task { await api.reconnectWorkBuddy() }
                    }
                    .buttonStyle(.borderless)
                    DataSourceRow(
                        name: "DeepSeek",
                        status: deepSeekStatus,
                        lastUpdate: updateText(api.status?.collection?.deepseek),
                        action: refresh
                    )
                    DataSourceRow(
                        name: "System",
                        status: systemStatus,
                        lastUpdate: updateText(api.status?.collection?.system),
                        action: refresh
                    )
                } else {
                    Text(settings.localized("Checking..."))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
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

    private var systemStatus: String {
        settings.localized(api.status?.system?.status ?? "Unavailable")
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
