import SwiftUI
import AppKit

struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var server: ServerManager
    @EnvironmentObject private var loginAtLaunch: LaunchAtLoginService
    @State private var connection = "未检查"
    @State private var pairingCode = ""
    @State private var busy = false
    @State private var notice = ""
    @State private var showRevoke = false

    var body: some View {
        Form {
            Section {
                Label("Magpie 显示伴侣", systemImage: "rectangle.on.rectangle")
                    .font(.title2.weight(.semibold))
                Text("账号、模型和调用由 Magpie 管理。AICC 只提供桌面小组件与墨水屏显示。")
                    .foregroundStyle(.secondary)
            }
            Section("Magpie 连接") {
                TextField("本机端口", value: $settings.magpiePort, format: .number.grouping(.never))
                HStack {
                    Text(connection).foregroundStyle(.secondary)
                    Spacer()
                    Button("检查连接") { Task { await checkConnection() } }.disabled(busy)
                    Button("打开 Magpie") {
                        NSWorkspace.shared.open(URL(fileURLWithPath: "/Applications/magpie.app"))
                    }
                }
            }
            Section("桌面小组件") {
                Text("在桌面添加 AICC 小组件，右键“编辑小组件”设置 Magpie 端口与账号数量。")
                Text("小组件独立读取 Magpie，关闭 AICC 后仍可更新。手动刷新只请求 Codex 额度。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("墨水屏连接") {
                Toggle("启用墨水屏桥接", isOn: $settings.bridgeEnabled)
                if settings.bridgeEnabled {
                    LabeledContent("桥接状态", value: server.isServerRunning ? "运行中" : "等待连接")
                    Text("Mac 与墨水屏需在同一可信网络。APK 自动发现电脑，输入下方配对码即可连接。")
                        .font(.caption).foregroundStyle(.secondary)
                    HStack {
                        Button("生成配对码") { Task { await pair() } }.disabled(!server.isServerRunning || busy)
                        if !pairingCode.isEmpty {
                            Text(pairingCode).font(.title3.monospacedDigit()).textSelection(.enabled)
                            Text("2 分钟内有效").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    Toggle("登录时启动桥接", isOn: Binding(get: { loginAtLaunch.isEnabled }, set: loginAtLaunch.setEnabled))
                    Button("撤销所有已配对设备", role: .destructive) { showRevoke = true }
                    if let error = loginAtLaunch.errorMessage { Text(error).font(.caption).foregroundStyle(.red) }
                }
                Text(settings.bridgeEnabled ? "关闭窗口后继续为墨水屏同步。" : "仅使用小组件时无需桥接，关闭窗口即退出 AICC。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("诊断") {
                HStack {
                    Button("查看日志") { NSWorkspace.shared.open(AICCPaths.logsDirectory) }
                    Button("项目与更新") {
                        NSWorkspace.shared.open(URL(string: "https://github.com/easonwong2026-del/AICC/releases")!)
                    }
                    Spacer()
                    Button("退出 AICC") { NSApp.terminate(nil) }
                }
                if !notice.isEmpty { Text(notice).font(.caption).foregroundStyle(.secondary) }
            }
        }
        .formStyle(.grouped)
        .frame(minWidth: 560, minHeight: 520)
        .onAppear { loginAtLaunch.refresh() }
        .onChange(of: settings.bridgeEnabled) { _, enabled in
            pairingCode = ""
            if enabled { server.startMonitoring() } else {
                server.stopMonitoring()
                server.stopServer(reason: .user)
                loginAtLaunch.setEnabled(false)
            }
        }
        .onChange(of: settings.magpiePort) { _, _ in
            connection = "未检查"
            pairingCode = ""
            if settings.bridgeEnabled { Task { _ = await server.restartServer() } }
        }
        .confirmationDialog("撤销设备后需要重新配对", isPresented: $showRevoke) {
            Button("撤销所有设备", role: .destructive) {
                Task {
                    server.stopMonitoring()
                    server.stopServer(reason: .user, waitForExit: true)
                    do {
                        try BridgeCredential.revoke()
                        pairingCode = ""
                        server.startMonitoring()
                        notice = "旧配对凭证已失效。"
                    } catch { notice = "无法更新配对凭证，请检查钥匙串权限。" }
                }
            }
        }
    }

    private func checkConnection() async {
        busy = true
        defer { busy = false }
        let snapshot = await WidgetStatusLoader.load(port: settings.magpiePort)
        if let snapshot {
            connection = "已连接 · Codex \(snapshot.codexAccounts?.count ?? 0) 个账号"
            if snapshot.codexStale { connection += " · 缓存" }
        } else { connection = "无法连接，请启动 Magpie 并检查端口。" }
    }

    private func pair() async {
        busy = true
        defer { busy = false }
        var request = URLRequest(url: URL(string: "http://127.0.0.1:8765/api/pairing")!)
        request.httpMethod = "POST"
        request.timeoutInterval = 5
        do {
            let (data, response) = try await WidgetStatusLoader.session.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200,
                  let payload = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let code = payload["code"] as? String else { throw URLError(.badServerResponse) }
            pairingCode = code
            Task {
                try? await Task.sleep(nanoseconds: 120_000_000_000)
                if pairingCode == code { pairingCode = "" }
            }
        } catch { notice = "无法生成配对码，请检查桥接状态。" }
    }
}
