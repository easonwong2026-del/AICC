import SwiftUI
import AppKit
import WidgetKit

struct SettingsView: View {
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var server: ServerManager
    @EnvironmentObject private var loginAtLaunch: LaunchAtLoginService
    @State private var connection = "未检查"
    @State private var pairingCode = ""
    @State private var busy = false
    @State private var notice = ""
    @State private var showRevoke = false
    @State private var widgetProviders: [String] = []
    @State private var showSelection = false
    // 与主控制台共用同一个助手实例，设置页不再单独起进程。
    @ObservedObject private var accountWorker = AccountWorkerManager.shared

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
                Picker("显示数量", selection: $settings.widgetCards) {
                    Text("自动").tag(WidgetCardDisplayMode.automatic)
                    Text("1 个").tag(WidgetCardDisplayMode.one)
                    Text("2 个").tag(WidgetCardDisplayMode.two)
                    Text("4 个").tag(WidgetCardDisplayMode.four)
                    Text("全部").tag(WidgetCardDisplayMode.all)
                }
                HStack {
                    Text("显示额度")
                    Spacer()
                    Text(selectionSummary).foregroundStyle(.secondary)
                    Button("选择额度…") { showSelection = true }
                }
                Text("可以勾选具体窗口（例如只显示 Gemini 的 7 天额度），勾选顺序即显示顺序。")
                Text("小尺寸最多 2 张、中尺寸最多 4 张额度卡片；一个账号的多个额度窗口合并显示。")
                Text("小组件独立读取 Magpie，关闭 AICC 后仍可更新。点击刷新会短暂运行刷新任务，不打开设置窗口。")
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
            Section("AI 账号与签到") {
                HStack {
                    Text("助手状态")
                    Spacer()
                    Text(accountWorkerSummary).foregroundStyle(accountWorkerIsHealthy ? .green : .secondary)
                    Button("检查助手") { Task { await accountWorker.refresh() } }
                }
                HStack {
                    Text("凭据存储")
                    Spacer()
                    Text(accountWorker.secureStoreSummary).foregroundStyle(.secondary)
                    Button("测试钥匙串") { Task { await accountWorker.checkSecureStore() } }
                }
                Picker(
                    "首选授权",
                    selection: Binding(
                        get: { accountWorker.preferredAuthSource },
                        set: { value in Task { await accountWorker.setPreferredAuthSource(value) } }
                    )
                ) {
                    Text("Magpie 优先").tag("magpie")
                    Text("自有优先").tag("own")
                }
                .pickerStyle(.segmented)
                Text("Magpie 优先：直接使用 Magpie 已登录的令牌，AICC 不刷新它。自有优先：AICC 用自己的扫码授权，并用自己的刷新令牌续期。优先级不可用时自动兜底到另一份。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if let error = accountWorker.authSettingsError {
                    Text(error).font(.caption).foregroundStyle(.red)
                }
                Text("助手是内嵌在本 App 内的独立可执行文件，只通过本机管道通信，不开放网络端口。")
                    .font(.caption).foregroundStyle(.secondary)
                Text("账号管理已移到主控制台（AI 账号 / 签到任务）。这里只保留助手与凭据的自检。")
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
        .onAppear {
            loginAtLaunch.refresh()
            Task { await accountWorker.refresh() }
        }
        .onChange(of: settings.bridgeEnabled) { _, enabled in
            pairingCode = ""
            if enabled { server.startMonitoring() } else {
                server.stopMonitoring()
                server.stopServer(reason: .user)
                loginAtLaunch.setEnabled(false)
            }
        }
        .onChange(of: settings.magpiePort) { _, _ in
            reloadWidgets()
            widgetProviders = WidgetStatusStore.load(port: settings.magpiePort)?.providers ?? []
            connection = "未检查"
            pairingCode = ""
            if settings.bridgeEnabled { Task { _ = await server.restartServer() } }
        }
        .onChange(of: settings.widgetCards) { _, _ in reloadWidgets() }
        .onChange(of: settings.widgetProvider) { _, _ in reloadWidgets() }
        .onChange(of: settings.widgetSelection) { _, _ in reloadWidgets() }
        .sheet(isPresented: $showSelection) {
            QuotaSelectionView(port: settings.magpiePort, selection: $settings.widgetSelection) { picked in
                settings.widgetSelection = picked
            }
        }
        .task { widgetProviders = WidgetStatusStore.load(port: settings.magpiePort)?.providers ?? [] }
        // 首选授权来源存在助手的 app_settings 里，进设置页时读一次真实值。
        .task { await accountWorker.loadAuthPreferences() }
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

    private var selectionSummary: String {
        settings.widgetSelection.isEmpty ? "自动" : "已选 \(settings.widgetSelection.count) 项"
    }

    private var accountWorkerSummary: String {
        switch accountWorker.state {
        case .stopped: return "未启动"
        case .starting: return "启动中…"
        case .running(let health): return health.summary
        case .failed(let message): return message.split(separator: "\n").first.map(String.init) ?? "失败"
        }
    }

    private var accountWorkerIsHealthy: Bool {
        if case .running(let health) = accountWorker.state { return health.isHealthy }
        return false
    }

    private func reloadWidgets() {
        settings.publishWidgetSettings(providers: WidgetStatusStore.load(port: settings.magpiePort)?.providers
            ?? widgetProviders)
        WidgetCenter.shared.reloadAllTimelines()
    }

    private func checkConnection() async {
        busy = true
        defer { busy = false }
        let snapshot = await WidgetStatusLoader.load(port: settings.magpiePort)
        if let snapshot {
            let providers = snapshot.providers.map(MagpieQuotaParser.providerName).joined(separator: "、")
            connection = "已连接 · \(providers.isEmpty ? "暂无服务商" : providers) · \(snapshot.accounts.count) 张额度卡"
            if snapshot.stale { connection += " · 缓存" }
            widgetProviders = snapshot.providers
            settings.publishWidgetSettings(providers: snapshot.providers)
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
