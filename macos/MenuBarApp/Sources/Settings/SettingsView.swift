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
    @State private var showCredentialRisk = false
    @State private var confirmDisableJSON = false
    // 客户端轮换时区：存在助手的轮换计划里，这里只是读写入口。
    @State private var rotationTimezone = "Asia/Shanghai"
    @State private var rotationTimezones: [String] = ["Asia/Shanghai", "UTC"]
    @State private var rotationRuleText = "错过开始时间 10 分钟以上不补跑；每次切换预留 5 分钟恢复余量。"
    @State private var rotationSettingsError: String?
    // 与主控制台共用同一个助手实例，设置页不再单独起进程。
    @ObservedObject private var accountWorker = AccountWorkerManager.shared

    var body: some View {
        Form {
            Section {
                Label("Magpie 显示伴侣", systemImage: "rectangle.on.rectangle")
                    .font(.title2.weight(.semibold))
                Text("账号、模型和调用由 Magpie 管理。AICC 在它之上管理账号与额度，"
                    + "跑签到与成长任务，并把看板推到桌面小组件和墨水屏。")
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
                    if let reason = server.runtimeError {
                        // 内置运行时缺失时桥接不会启动，也不会退回系统 Python：
                        // 界面必须说明原因，否则用户只看到"连不上"。
                        Text(reason).font(.caption).foregroundStyle(.red)
                    }
                    Toggle("允许局域网设备连接", isOn: $settings.bridgeLANEnabled)
                    if settings.bridgeLANEnabled {
                        lanStatus
                    } else {
                        Text("默认只监听本机（127.0.0.1），墨水屏与 Android 连不上，只用小组件时够用。要让墨水屏连接，必须在这里显式打开。")
                            .font(.caption).foregroundStyle(.secondary)
                    }
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
                Text("只作用于 AICC 后台请求（签到、额度、成长查询），**不决定 WorkBuddy 客户端登录哪个账号**。"
                    + "客户端切换固定优先自有长期授权（带刷新令牌），没有时才借用 Magpie 的临时访问令牌。"
                    + "Magpie 优先：直接用 Magpie 的令牌，AICC 不刷新它；自有优先：用自己的扫码授权并自行续期。"
                    + "首选那份不可用时自动兜底到另一份。")
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
            Section("WorkBuddy 客户端") {
                Picker("账号轮换时区", selection: Binding(
                    get: { rotationTimezone },
                    set: { value in
                        rotationTimezone = value
                        Task { await saveRotationTimezone(value) }
                    }
                )) {
                    ForEach(rotationTimezones, id: \.self) { zone in
                        Text(zone).tag(zone)
                    }
                }
                Text("轮换的开始与结束时间按这个时区解释。只提供固定偏移、无夏令时的时区，避免半夜算错一小时。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(rotationRuleText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if let rotationSettingsError {
                    Text(rotationSettingsError).font(.caption).foregroundStyle(.red)
                }
                Text("客户端登录的切换与恢复在主控制台的「AI 账号」页操作。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("凭据存储") {
                HStack {
                    Text("当前方式")
                    Spacer()
                    Text(credentialStorageHeadline)
                        .foregroundStyle(credentialStorageEnabled ? .orange : .secondary)
                }
                Text(credentialStorageSummary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if credentialStorage == nil {
                    // 老助手根本没有本地 JSON：不要显示一个永远点不动的开关。
                    Text("这个版本的内嵌助手还没有本地 JSON 存储。更新 AICC 之后可以在这里切换。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else if credentialStorageEnabled {
                    Text("新建授权（WorkBuddy 与 Qoder 自有授权）会写入本机 JSON（明文）。已有账号保持原来的存储，不会自动迁移。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Button("改回钥匙串…") { confirmDisableJSON = true }
                } else {
                    Text("新授权默认写入 macOS 钥匙串。启用本地 JSON 后，后台任务不会再因为 App 重新签名而弹出授权框。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Button("启用本地 JSON…") { showCredentialRisk = true }
                        .disabled(!accountWorkerIsHealthy)
                }
                if credentialStorageNeedsConsent {
                    Text("尚未确认明文存储的风险：在启用之前，AICC 不会把任何新令牌写成明文文件。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let error = accountWorker.authSettingsError {
                    Text(error).font(.caption).foregroundStyle(.red)
                }
                HStack {
                    Text("钥匙串自检")
                    Spacer()
                    Text(accountWorker.secureStoreSummary).foregroundStyle(.secondary)
                    Button("测试钥匙串") { Task { await accountWorker.checkSecureStore() } }
                }
            }
            CredentialMigrationSection(worker: accountWorker)
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
        .onChange(of: settings.bridgeLANEnabled) { _, enabled in
            // 改监听范围必须真的重启进程：只改开关不动绑定，等于界面说"已关闭"
            // 而端口还开着。
            pairingCode = ""
            guard settings.bridgeEnabled else { return }
            Task {
                if enabled {
                    // 先有证书再有监听：桥接不会在没有证书时退回明文。
                    _ = await accountWorker.ensureBridgeTls()
                }
                _ = await server.restartServer()
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
        .sheet(isPresented: $showCredentialRisk) {
            PlaintextCredentialSheet(
                risk: credentialStorage?.risk ?? "",
                onConfirm: { await accountWorker.setJSONCredentialStorage(true) },
                onCancel: {}
            )
        }
        .confirmationDialog(
            "之后新建的授权会重新使用钥匙串",
            isPresented: $confirmDisableJSON
        ) {
            Button("改回钥匙串") {
                Task { _ = await accountWorker.setJSONCredentialStorage(false) }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("已经写成 JSON 的授权不受影响，继续可用；不会自动迁移回钥匙串。")
        }
        .sheet(isPresented: $showSelection) {
            QuotaSelectionView(port: settings.magpiePort, selection: $settings.widgetSelection) { picked in
                settings.widgetSelection = picked
            }
        }
        .task { widgetProviders = WidgetStatusStore.load(port: settings.magpiePort)?.providers ?? [] }
        // 首选授权来源存在助手的 app_settings 里，进设置页时读一次真实值。
        .task { await accountWorker.loadAuthPreferences() }
        .task { await loadRotationSettings() }
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

    /// 助手给出的凭据存储现状；老助手没有这个字段时为 nil。
    private var credentialStorage: CredentialStoragePolicy? {
        accountWorker.credentialStorage
    }

    private var credentialStorageEnabled: Bool {
        credentialStorage?.jsonEnabled ?? false
    }

    private var credentialStorageNeedsConsent: Bool {
        credentialStorage?.consentRequired ?? false
    }

    private var credentialStorageHeadline: String {
        credentialStorage?.headline ?? "未获取"
    }

    private var credentialStorageSummary: String {
        guard let policy = credentialStorage else {
            return "助手未报告各来源的授权数量。"
        }
        return policy.summary
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

    /// 读一次轮换计划，只为拿到时区与后端给的规则常量。
    private func loadRotationSettings() async {
        do {
            let status = try await accountWorker.workbuddyRotationStatus()
            rotationTimezone = status.plan.timezone
            if !status.supportedTimezones.isEmpty {
                rotationTimezones = status.supportedTimezones
            }
            let late = max(1, status.lateStartToleranceSeconds / 60)
            let reserve = max(1, status.restoreReserveSeconds / 60)
            rotationRuleText = "固定规则：错过开始时间 \(late) 分钟以上，本轮不补跑；"
                + "每次切换前预留 \(reserve) 分钟恢复余量。"
            rotationSettingsError = nil
        } catch {
            rotationSettingsError = "读不到轮换设置：\(error.localizedDescription)"
        }
    }

    private func saveRotationTimezone(_ zone: String) async {
        do {
            _ = try await accountWorker.workbuddyRotationSave(params: ["timezone": zone])
            rotationSettingsError = nil
        } catch {
            rotationSettingsError = "时区没保存成功：\(error.localizedDescription)"
            await loadRotationSettings()
        }
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

    /// The LAN section is its own view: the type checker gives up on the whole
    /// settings body when all of this is inlined.
    @ViewBuilder private var lanStatus: some View {
        LabeledContent("HTTPS 服务", value: server.lanServing ? "运行中（TLS 1.3 优先）" : "未启动")
        if let tls = accountWorker.bridgeTls, let expiry = tls.notAfter {
            LabeledContent("证书有效期", value: expiry)
        }
        if let tls = accountWorker.bridgeTls, !tls.groupedFingerprint.isEmpty {
            Text("Mac 证书指纹").font(.caption).foregroundStyle(.secondary)
            Text(tls.groupedFingerprint)
                .font(.system(.caption, design: .monospaced))
                .textSelection(.enabled)
            Text("在墨水屏上配对时，它显示的指纹必须与上面这段完全一致。指纹来自根证书，叶证书续期不会改变它，因此不用重新配对。")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("复制指纹") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(tls.spki ?? "", forType: .string)
                }
                Button("重置证书身份", role: .destructive) { Task { await resetIdentity() } }
            }
            Text("重置会换掉根证书，所有已配对设备都必须重新确认新指纹；不重置就无法让旧设备失效。")
                .font(.caption).foregroundStyle(.orange)
        } else if let error = accountWorker.bridgeTlsError {
            Text(error).font(.caption).foregroundStyle(.red)
        }
        if !server.lanServing {
            Text("局域网已打开，但 HTTPS 监听没有起来（通常是证书还没准备好）。墨水屏此时连不上，也不会退回明文 HTTP。")
                .font(.caption).foregroundStyle(.orange)
        }
    }

    /// Replace the certificate root, then restart the listener so the port
    /// actually serves the new identity — a reset that leaves the old
    /// certificate running would look successful and change nothing.
    private func resetIdentity() async {
        busy = true
        defer { busy = false }
        let replaced = await accountWorker.resetBridgeTls()
        guard replaced else { return }
        pairingCode = ""
        if !(await server.restartServer()) {
            notice = "证书已重置，但桥接重启失败：端口上仍是旧证书，请手动重启 AICC"
        }
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
