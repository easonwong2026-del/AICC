import AppKit
import Combine
import SwiftUI
import WidgetKit

/// 设置窗口的全部交互状态与动作。
///
/// 视图只读它、调它；所有"该不该做、做了会怎样"的决策都在
/// `Models/SettingsCenter.swift` 的纯值里，这里只负责把 effect 落到真实对象上。
@MainActor
final class SettingsModel: ObservableObject {
    let settings: AppSettings
    let server: ServerManager
    let loginAtLaunch: LaunchAtLoginService
    let worker: AccountWorkerManager

    /// Magpie 连接探测的最近结论。只在用户点「检查连接」后改变。
    @Published var connection = "未检查"
    @Published var pairingCode = ""
    /// 连接与设备组的通用忙：检查连接、生成配对码、撤销设备。
    @Published private(set) var busy = false
    /// 设备安全操作的一句话结论，贴着操作显示。
    @Published var securityNotice: String?

    @Published var widgetProviders: [String] = []
    @Published var showQuotaSelection = false
    @Published var showCredentialRisk = false
    @Published var confirmDisableJSON = false
    @Published var confirmRevokeDevices = false
    @Published var showCredentialDetail = false
    @Published var showBridgeSecurity = false

    @Published var rotationTimezone = ""
    @Published var rotationTimezones: [String] = []
    @Published var rotationRule = ""
    @Published var rotationError: String?
    @Published var rotationLoaded = false

    @Published var tlsReset: SettingsTLSResetFlow = .idle
    /// 凭据存储开关自己的失败原因。与首选授权的 `authSettingsError` 分开，
    /// 否则同一条错误会在两个分组里各显示一次。
    @Published var credentialStorageError: String?

    /// 分类选择。切换只改值并记住选择，没有任何业务副作用。
    let navigation: SettingsNavigation
    private var navigationObserver: AnyCancellable?

    /// 一次性只读加载：切分类不会重跑。
    private var loadGate = SettingsLoadGate()

    init(
        settings: AppSettings,
        server: ServerManager,
        loginAtLaunch: LaunchAtLoginService,
        worker: AccountWorkerManager,
        navigation: SettingsNavigation = SettingsNavigation()
    ) {
        self.settings = settings
        self.server = server
        self.loginAtLaunch = loginAtLaunch
        self.worker = worker
        self.navigation = navigation
        // 转发导航的变更，视图只订阅 model 一个对象。
        navigationObserver = navigation.objectWillChange.sink { [weak self] in
            self?.objectWillChange.send()
        }
    }

    var category: SettingsCategory { navigation.category }

    func selectCategory(_ category: SettingsCategory) {
        navigation.select(category)
    }

    // MARK: - 派生状态（视图只读这些，不自己判断）

    var bridgePhase: SettingsBridgePhase {
        SettingsBridgePhase.resolve(
            enabled: settings.bridgeEnabled,
            running: server.isServerRunning,
            lanEnabled: settings.bridgeLANEnabled,
            lanServing: server.lanServing,
            runtimeError: server.runtimeError
        )
    }

    var lanPhase: SettingsLANPhase {
        SettingsLANPhase.resolve(enabled: settings.bridgeLANEnabled, lanServing: server.lanServing)
    }

    var credentialView: SettingsCredentialView {
        // 助手没跑起来是"暂时读不到"，老助手没有字段是"不支持"——两者文案不同。
        SettingsCredentialView.resolve(worker.credentialStorage, helperRunning: workerPhase.isRunning)
    }

    var credentialAttention: SettingsCredentialAttention {
        SettingsCredentialAttention.resolve(worker.credentialMigration)
    }

    var workerPhase: SettingsWorkerPhase {
        SettingsWorkerPhase.resolve(worker.state)
    }

    var tlsPhase: SettingsTLSResetFlow { tlsReset }

    var isBusy: Bool { busy || tlsReset.isBusy || worker.credentialMigrationBusy }

    // MARK: - 首次加载（只读）

    /// 顺序：先把助手拉起来，再并行读三份只读配置。
    ///
    /// 旧实现把 `loadAuthPreferences()` 和 `refresh()` 放在同一批 `.task` 里，
    /// 助手还没起来时两个读取会静默 return，界面就停在默认值。这里先等 refresh。
    func loadOnce() async {
        #if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
        return
        #else
        guard loadGate.shouldRun() else { return }
        loginAtLaunch.refresh()
        await worker.refresh()

        async let widgets: Void = refreshWidgetProviders()
        async let auth: Void = worker.loadAuthPreferences()
        async let rotation: Void = loadRotationSettings()
        async let migration: Void = worker.refreshCredentialMigration()
        _ = await (widgets, auth, rotation, migration)
        #endif
    }

    func refreshWidgetProviders() async {
        widgetProviders = WidgetStatusStore.load(port: settings.magpiePort)?.providers ?? []
    }

    // MARK: - 生命周期副作用（由 shell 的 onChange 调用）

    /// 把一次设置变化落到真实服务。导航不在这里，所以切分类永远不会触发它。
    func handle(_ trigger: SettingsTrigger) {
        let effects = SettingsActionPlan.effects(for: trigger)
        var deferred: [SettingsEffect] = []
        for effect in effects {
            switch effect {
            case .ensureBridgeTLS, .restartBridge:
                deferred.append(effect)
            default:
                apply(effect)
            }
        }
        guard !deferred.isEmpty else { return }
        Task { @MainActor in
            for effect in deferred {
                await applyAsync(effect)
            }
        }
    }

    private func apply(_ effect: SettingsEffect) {
        switch effect {
        case .clearPairingCode:
            pairingCode = ""
        case .startBridgeMonitoring:
            server.startMonitoring()
        case .stopBridgeMonitoring:
            server.stopMonitoring()
        case .stopBridgeService:
            server.stopServer(reason: .user)
        case .disableLaunchAtLogin:
            loginAtLaunch.setEnabled(false)
        case .reloadWidgetDisplay:
            reloadWidgetDisplay()
        case .resetConnectionProbe:
            connection = "未检查"
        case .ensureBridgeTLS, .restartBridge:
            break
        }
    }

    private func applyAsync(_ effect: SettingsEffect) async {
        switch effect {
        case .ensureBridgeTLS:
            // 先有证书再有监听：桥接不会在没有证书时退回明文。
            _ = await worker.ensureBridgeTls()
        case .restartBridge:
            _ = await server.restartServer()
        default:
            break
        }
    }

    private func reloadWidgetDisplay() {
        widgetProviders = WidgetStatusStore.load(port: settings.magpiePort)?.providers ?? widgetProviders
        settings.publishWidgetSettings(providers: widgetProviders)
        WidgetCenter.shared.reloadAllTimelines()
    }

    // MARK: - Magpie

    func checkConnection() async {
        busy = true
        defer { busy = false }
        let snapshot = await WidgetStatusLoader.load(port: settings.magpiePort)
        if let snapshot {
            let providers = snapshot.providers.map(MagpieQuotaParser.providerName).joined(separator: "、")
            connection = "已连接 · \(providers.isEmpty ? "暂无服务商" : providers) · \(snapshot.accounts.count) 张额度卡"
            if snapshot.stale { connection += " · 缓存" }
            widgetProviders = snapshot.providers
            settings.publishWidgetSettings(providers: snapshot.providers)
        } else {
            connection = "无法连接，请启动 Magpie 并检查端口。"
        }
    }

    func openMagpie() {
        NSWorkspace.shared.open(URL(fileURLWithPath: "/Applications/magpie.app"))
    }

    // MARK: - 墨水屏

    func generatePairingCode() async {
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
            securityNotice = nil
            Task {
                try? await Task.sleep(nanoseconds: 120_000_000_000)
                if pairingCode == code { pairingCode = "" }
            }
        } catch {
            securityNotice = "无法生成配对码，请检查桥接状态。"
        }
    }

    func revokePairedDevices() async {
        busy = true
        defer { busy = false }
        server.stopMonitoring()
        server.stopServer(reason: .user, waitForExit: true)
        do {
            try BridgeCredential.revoke()
            pairingCode = ""
            server.startMonitoring()
            securityNotice = "旧配对凭证已失效，所有设备需要重新配对。"
        } catch {
            securityNotice = "无法更新配对凭证，请检查钥匙串权限。"
        }
    }

    // MARK: - 证书身份

    func requestTLSReset() {
        guard tlsReset.request() else { return }
    }

    func cancelTLSReset() {
        tlsReset.cancel()
    }

    func confirmTLSReset() async {
        guard tlsReset.confirm() else { return }
        busy = true
        defer { busy = false }
        securityNotice = nil
        let replaced = await worker.resetBridgeTls()
        pairingCode = ""
        var restarted = false
        if replaced {
            restarted = await server.restartServer()
        }
        tlsReset.finish(
            identityReplaced: replaced,
            restartSucceeded: restarted,
            error: worker.bridgeTlsError
        )
        if replaced && restarted {
            securityNotice = "证书身份已更换。所有已配对设备需要重新确认新指纹。"
        }
    }

    func copyFingerprint() {
        guard let spki = worker.bridgeTls?.spki, !spki.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(spki, forType: .string)
        securityNotice = "指纹已复制。"
    }

    // MARK: - 凭据存储

    func setJSONCredentialStorage(_ enabled: Bool) async -> Bool {
        let saved = await worker.setJSONCredentialStorage(enabled)
        let outcome = SettingsCredentialSwitchOutcome.resolve(
            saved: saved,
            desired: enabled,
            error: worker.authSettingsError
        )
        credentialStorageError = outcome.failure
        return outcome.showsSuccess
    }

    func checkSecureStore() async {
        await worker.checkSecureStore()
    }

    // MARK: - 首选授权

    func setPreferredAuthSource(_ value: String) async {
        await worker.setPreferredAuthSource(value)
    }

    // MARK: - 轮换时区

    private func loadRotationSettings() async {
        rotationLoaded = false
        do {
            let status = try await worker.workbuddyRotationStatus()
            rotationTimezone = status.plan.timezone
            if !status.supportedTimezones.isEmpty {
                rotationTimezones = status.supportedTimezones
            }
            let late = max(1, status.lateStartToleranceSeconds / 60)
            let reserve = max(1, status.restoreReserveSeconds / 60)
            rotationRule = "错过开始时间 \(late) 分钟以上本轮不补跑；每次切换前预留 \(reserve) 分钟恢复余量。"
            rotationError = nil
            rotationLoaded = true
        } catch {
            rotationError = "读不到轮换设置：\(Self.describe(error))"
            rotationLoaded = true
        }
    }

    static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}
