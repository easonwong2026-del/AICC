import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var ocx: OpenCodexController
    @EnvironmentObject var api: APIService

    var body: some View {
        TabView {
            GeneralSettingsView()
                .tabItem { Label("General", systemImage: "switch.2") }
            DataSourcesSettingsView()
                .tabItem { Label("Data Sources", systemImage: "antenna.radiowaves.left.and.right") }
            OpenCodexSettingsView()
                .tabItem { Label("OpenCodex", systemImage: "server.rack") }
            EInkSettingsView()
                .tabItem { Label("E-ink", systemImage: "display") }
            AdvancedSettingsView()
                .tabItem { Label("Advanced", systemImage: "wrench.and.screwdriver") }
        }
        .frame(width: 500, height: 400)
    }
}

// MARK: - General

struct GeneralSettingsView: View {
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Form {
            Section {
                Toggle("Launch AICC at login", isOn: $settings.launchAtLogin)
                Toggle("Show icon in menu bar", isOn: $settings.menuBarShowIcon)
            } header: {
                Text("Startup")
            }

            Section {
                Picker("Auto-refresh interval", selection: $settings.autoRefreshInterval) {
                    Text("5 seconds").tag(5.0)
                    Text("10 seconds").tag(10.0)
                    Text("30 seconds").tag(30.0)
                    Text("60 seconds").tag(60.0)
                }
                Toggle("Enable notifications", isOn: $settings.enableNotifications)
            } header: {
                Text("Refresh & Notifications")
            }

            Section {
                Picker("Theme", selection: $settings.themeMode) {
                    Text("System").tag("system")
                    Text("Light").tag("light")
                    Text("Dark").tag("dark")
                }
            } header: {
                Text("Appearance")
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Data Sources

struct DataSourcesSettingsView: View {
    @EnvironmentObject var api: APIService

    var body: some View {
        Form {
            Section {
                DataSourceRow(name: "Codex", status: codexStatus, lastUpdate: "Auto via ChatGPT", action: {})
                DataSourceRow(name: "WorkBuddy", status: wbStatus, lastUpdate: "Auto via CDP", action: {})
                DataSourceRow(name: "DeepSeek", status: dsStatus, lastUpdate: "API balance check", action: {})
            } header: {
                Text("Connected Services")
            }
        }
        .formStyle(.grouped)
    }

    private var codexStatus: String {
        api.status?.codex?.five_hour?.remaining != nil ? "Connected" : "Unavailable"
    }

    private var wbStatus: String {
        api.status?.workbuddy?.points != nil ? "Connected" : "Unavailable"
    }

    private var dsStatus: String {
        api.status?.deepseek?.status ?? "Unknown"
    }
}

struct DataSourceRow: View {
    let name: String
    let status: String
    let lastUpdate: String
    let action: () -> Void

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(name).font(.system(size: 13, weight: .medium))
                Text(lastUpdate).font(.system(size: 10)).foregroundColor(.secondary)
            }
            Spacer()
            HStack(spacing: 8) {
                Circle()
                    .fill(statusColor)
                    .frame(width: 6, height: 6)
                Text(status)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                Button("Refresh") { action() }
                    .buttonStyle(.plain)
                    .font(.system(size: 10))
                    .foregroundColor(.accentColor)
            }
        }
    }

    private var statusColor: Color {
        status == "Connected" || status == "Online" ? .green : .secondary
    }
}

// MARK: - OpenCodex

struct OpenCodexSettingsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var ocx: OpenCodexController

    var body: some View {
        Form {
            Section {
                Toggle("Start OpenCodex with Codex Desktop", isOn: $settings.ocxAutoStart)
                Toggle("Stop OpenCodex when Codex quits", isOn: $settings.ocxStopOnCodexExit)
                Toggle("Wait for proxy before starting Codex", isOn: $settings.ocxWaitProxy)
            } header: {
                Text("Auto-launch")
            }

            Section {
                HStack {
                    Text("OCX Path")
                    Spacer()
                    Text(ocx.detectedPath ?? "Not detected")
                        .font(.system(size: 10))
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Button("Redetect") {
                        Task { await ocx.detectExecutable() }
                    }
                    .buttonStyle(.plain)
                    .font(.system(size: 10))
                    .foregroundColor(.accentColor)
                }
                TextField("Service address", text: $settings.ocxServiceAddress)
                    .font(.system(size: 12))
            } header: {
                Text("Path & Connection")
            }

            Section {
                HStack(spacing: 12) {
                    Button("Open Dashboard") { ocx.openDashboard() }
                        .buttonStyle(.bordered)
                    Button("Restart OpenCodex") {
                        Task { await ocx.restart() }
                    }
                    .buttonStyle(.bordered)
                    Button("Run Diagnostics") { runDiagnostics() }
                        .buttonStyle(.bordered)
                }
            } header: {
                Text("Actions")
            }
        }
        .formStyle(.grouped)
    }

    private func runDiagnostics() {}
}

// MARK: - E-ink

struct EInkSettingsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var api: APIService

    var body: some View {
        Form {
            Section {
                HStack {
                    Text("Sync Status")
                    Spacer()
                    Text(api.status?.system?.status ?? "Unknown")
                        .foregroundColor(.secondary)
                }
                HStack {
                    Text("Sync Interval")
                    Spacer()
                    Picker("", selection: $settings.einkSyncInterval) {
                        Text("15s").tag(15.0)
                        Text("30s").tag(30.0)
                        Text("60s").tag(60.0)
                    }
                    .labelsHidden()
                }
                if let updated = api.lastRefresh {
                    HStack {
                        Text("Last Sync")
                        Spacer()
                        Text(updated, style: .time)
                            .foregroundColor(.secondary)
                    }
                }
            } header: {
                Text("Synchronization")
            }

            Section {
                Button("Sync Now") { Task { await api.fetchStatus(force: true) } }
                HStack {
                    Text("Display Template")
                    Text("Default").foregroundColor(.secondary)
                }
                HStack {
                    Text("Device Config")
                    Text("Auto-detected").foregroundColor(.secondary)
                }
            } header: {
                Text("Configuration")
            }
        }
        .formStyle(.grouped)
    }
}

// MARK: - Advanced

struct AdvancedSettingsView: View {
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Form {
            Section {
                Button("View Logs") {
                    let logsURL = NSHomeDirectory() + "/Library/Logs/AICC-Dashboard"
                    NSWorkspace.shared.open(URL(fileURLWithPath: logsURL))
                }
                Button("Show Data Directory") {
                    let dataURL = NSHomeDirectory() + "/Library/Application Support/AICC-Dashboard/data"
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: dataURL)])
                }
            } header: {
                Text("Diagnostics")
            }

            Section {
                Button("Restart Data Service") {
                    // Send restart command to server
                }
                Button("Export Diagnostic Info") {}
            } header: {
                Text("Service")
            }

            Section {
                Button("Clear Cache") {}
            } header: {
                Text("Maintenance")
            }
        }
        .formStyle(.grouped)
    }
}
