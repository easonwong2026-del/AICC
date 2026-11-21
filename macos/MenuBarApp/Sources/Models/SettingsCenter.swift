import Combine
import Foundation

// MARK: - 设置中心的信息架构
//
// 这个文件只放**纯值**：分类、设置项映射、状态派生与动作决策。
// 视图不自己推断状态，也不在 body 里内联 if/else 决定文案；
// 所有"看起来是什么"与"点了会怎样"都在这里可测。

/// 设置窗口的四个分类。顺序即导航顺序。
enum SettingsCategory: String, CaseIterable, Identifiable {
    case general
    case connection
    case credentials
    case advanced

    var id: String { rawValue }

    var title: String {
        switch self {
        case .general: return "常规"
        case .connection: return "连接与设备"
        case .credentials: return "授权与存储"
        case .advanced: return "高级与诊断"
        }
    }

    var symbol: String {
        switch self {
        case .general: return "slider.horizontal.3"
        case .connection: return "antenna.radiowaves.left.and.right"
        case .credentials: return "key.horizontal"
        case .advanced: return "stethoscope"
        }
    }

    var items: [SettingsItemKey] {
        switch self {
        case .general:
            return [.widgetCardCount, .widgetQuotaSelection, .projectUpdates]
        case .connection:
            return [
                .magpiePort, .magpieConnectionCheck, .magpieOpen,
                .bridgeEnabled, .bridgeStatus, .bridgeRuntimeError,
                .bridgeLAN, .bridgeHTTPSStatus, .bridgePairingCode,
                .loginAtLaunch,
                .bridgeCertificateExpiry, .bridgeFingerprint,
                .bridgeFingerprintCopy, .bridgeIdentityReset,
                .bridgeTLSFailure, .revokePairedDevices,
            ]
        case .credentials:
            return [
                .preferredAuthSource, .credentialStorageMode,
                .credentialStorageSummary, .enableJSONStorage,
                .disableJSONStorage, .plaintextConsentMissing,
                .credentialSettingsError, .secureStoreSelfCheck,
                .secureStoreProbe, .credentialManagementEntry,
            ]
        case .advanced:
            return [
                .workerStatus, .workerCheck,
                .rotationTimezone, .rotationRule, .rotationError,
                .tlsDiagnostics, .openLogs, .quitApp,
            ]
        }
    }
}

/// 每个设置项的稳定标识。测试用它检查"一个不漏、一个不重"。
enum SettingsItemKey: String, CaseIterable, Identifiable {
    // 常规
    case widgetCardCount
    case widgetQuotaSelection
    case projectUpdates

    // 连接与设备
    case magpiePort
    case magpieConnectionCheck
    case magpieOpen
    case bridgeEnabled
    case bridgeStatus
    case bridgeRuntimeError
    case bridgeLAN
    case bridgeHTTPSStatus
    case bridgePairingCode
    case loginAtLaunch
    case bridgeCertificateExpiry
    case bridgeFingerprint
    case bridgeFingerprintCopy
    case bridgeIdentityReset
    case bridgeTLSFailure
    case revokePairedDevices

    // 授权与存储
    case preferredAuthSource
    case credentialStorageMode
    case credentialStorageSummary
    case enableJSONStorage
    case disableJSONStorage
    case plaintextConsentMissing
    case credentialSettingsError
    case secureStoreSelfCheck
    case secureStoreProbe
    case credentialManagementEntry

    // 高级与诊断
    case workerStatus
    case workerCheck
    case rotationTimezone
    case rotationRule
    case rotationError
    case tlsDiagnostics
    case openLogs
    case quitApp

    var id: String { rawValue }

    /// 审计文档与测试用的可读名称，与旧设置页文案一一对应。
    var title: String {
        switch self {
        case .widgetCardCount: return "小组件显示数量"
        case .widgetQuotaSelection: return "小组件显示额度"
        case .projectUpdates: return "项目与更新"
        case .magpiePort: return "Magpie 本机端口"
        case .magpieConnectionCheck: return "检查 Magpie 连接"
        case .magpieOpen: return "打开 Magpie"
        case .bridgeEnabled: return "启用墨水屏桥接"
        case .bridgeStatus: return "桥接运行状态"
        case .bridgeRuntimeError: return "桥接运行时错误"
        case .bridgeLAN: return "允许局域网设备连接"
        case .bridgeHTTPSStatus: return "HTTPS 服务状态"
        case .bridgePairingCode: return "生成配对码"
        case .loginAtLaunch: return "登录时启动桥接"
        case .bridgeCertificateExpiry: return "证书有效期"
        case .bridgeFingerprint: return "证书指纹"
        case .bridgeFingerprintCopy: return "复制指纹"
        case .bridgeIdentityReset: return "重置证书身份"
        case .bridgeTLSFailure: return "证书准备失败"
        case .revokePairedDevices: return "撤销所有已配对设备"
        case .preferredAuthSource: return "首选授权来源"
        case .credentialStorageMode: return "当前凭据存储方式"
        case .credentialStorageSummary: return "凭据来源分布"
        case .enableJSONStorage: return "启用本地 JSON"
        case .disableJSONStorage: return "改回钥匙串"
        case .plaintextConsentMissing: return "明文存储未确认"
        case .credentialSettingsError: return "凭据设置错误"
        case .secureStoreSelfCheck: return "钥匙串自检结果"
        case .secureStoreProbe: return "测试钥匙串"
        case .credentialManagementEntry: return "管理凭据"
        case .workerStatus: return "账号助手状态"
        case .workerCheck: return "检查账号助手"
        case .rotationTimezone: return "账号轮换时区"
        case .rotationRule: return "轮换规则"
        case .rotationError: return "轮换设置读取失败"
        case .tlsDiagnostics: return "TLS 诊断信息"
        case .openLogs: return "查看日志"
        case .quitApp: return "退出 AICC"
        }
    }

    var category: SettingsCategory {
        for category in SettingsCategory.allCases where category.items.contains(self) {
            return category
        }
        // 新加 key 忘了登记时快速失败：编译期做不到，就让测试与调试断言炸出来。
        assertionFailure("SettingsItemKey.\(rawValue) 未登记到任何分类")
        return .general
    }
}

// MARK: - 只读状态的展示派生

/// 与 ConsoleTheme.Tone 同义，但模型层不依赖 SwiftUI / 控制台目录。
enum SettingsTone: Equatable {
    case neutral
    case ok
    case warning
    case error
    case accent
}

/// 桥接（墨水屏）整体状态。
///
/// 顺序与 `ServerManager` 的真实语义一致：没有运行时 → 失败；开关关 → 关闭；
/// 进程没起 → 等待；局域网开关开了但没有 HTTPS 监听 → 异常；否则正常。
enum SettingsBridgePhase: Equatable {
    case off
    case waiting
    case loopbackOnly
    case lanServing
    case lanIntentWithoutListener
    case failed(String)

    static func resolve(
        enabled: Bool,
        running: Bool,
        lanEnabled: Bool,
        lanServing: Bool,
        runtimeError: String?
    ) -> SettingsBridgePhase {
        guard enabled else { return .off }
        if let runtimeError, !running { return .failed(runtimeError) }
        guard running else { return .waiting }
        if lanEnabled && !lanServing { return .lanIntentWithoutListener }
        return lanServing ? .lanServing : .loopbackOnly
    }

    var statusText: String {
        switch self {
        case .off: return "已关闭"
        case .waiting: return "等待连接"
        case .loopbackOnly: return "运行中 · 仅本机"
        case .lanServing: return "运行中 · 局域网 HTTPS"
        case .lanIntentWithoutListener: return "运行中 · 仅本机"
        case .failed: return "未能启动"
        }
    }

    var tone: SettingsTone {
        switch self {
        case .off, .waiting: return .neutral
        case .loopbackOnly, .lanServing: return .ok
        case .lanIntentWithoutListener: return .warning
        case .failed: return .error
        }
    }

    var failureReason: String? {
        if case .failed(let reason) = self { return reason }
        return nil
    }

    /// 只在异常时出现的行内说明。正常状态不占高度。
    var warningText: String? {
        switch self {
        case .lanIntentWithoutListener:
            return "局域网已打开，但 HTTPS 监听没有起来，通常是证书还没准备好。墨水屏此时连不上，也不会退回明文 HTTP。"
        case .failed:
            return "内置运行时缺失或无法启动，桥接与局域网都不可用。"
        default:
            return nil
        }
    }
}

/// 局域网那一行的状态。
enum SettingsLANPhase: Equatable {
    case off
    case served
    case allowedWithoutListener

    static func resolve(enabled: Bool, lanServing: Bool) -> SettingsLANPhase {
        guard enabled else { return .off }
        return lanServing ? .served : .allowedWithoutListener
    }

    var text: String {
        switch self {
        case .off: return "仅本机"
        case .served: return "局域网 HTTPS"
        case .allowedWithoutListener: return "已允许，未监听"
        }
    }

    var tone: SettingsTone {
        switch self {
        case .off: return .neutral
        case .served: return .ok
        case .allowedWithoutListener: return .warning
        }
    }
}

/// 凭据存储的一句话结论 + 该显示哪个动作。
struct SettingsCredentialView: Equatable {
    enum Mode: Equatable {
        case keychain
        case localJSON
        /// 老助手没有 `credentials` 字段。
        case unknown
    }

    var mode: Mode
    var headline: String
    var detail: String
    var tone: SettingsTone
    var needsConsent: Bool

    static func resolve(_ policy: CredentialStoragePolicy?, helperRunning: Bool = true) -> SettingsCredentialView {
        guard let policy else {
            // 两种"读不到"必须分开：助手没跑是暂时的，老版本没有字段是永久的。
            return SettingsCredentialView(
                mode: .unknown,
                headline: helperRunning ? "未获取" : "无法读取",
                detail: helperRunning
                    ? "这个版本的助手还没有报告凭据存储方式。"
                    : "账号助手未运行，暂时读不到存储方式。",
                tone: .neutral,
                needsConsent: false
            )
        }
        return SettingsCredentialView(
            mode: policy.jsonEnabled ? .localJSON : .keychain,
            headline: policy.headline,
            detail: policy.summary,
            tone: policy.jsonEnabled ? .warning : .neutral,
            needsConsent: policy.consentRequired
        )
    }
}

/// 需要处理的凭据问题数量。主页只显示这一行，明细进详情。
struct SettingsCredentialAttention: Equatable {
    var migratable: Int = 0
    var blocked: Int = 0
    var needsRelink: Int = 0
    var keyMigrationFailed: Int = 0

    static func resolve(_ state: CredentialMigrationState?) -> SettingsCredentialAttention {
        guard let state else { return SettingsCredentialAttention() }
        return SettingsCredentialAttention(
            migratable: state.summary.migratable,
            blocked: state.summary.blocked,
            needsRelink: state.summary.needsRelink,
            keyMigrationFailed: state.keyMigration.failed
        )
    }

    var total: Int { migratable + blocked + needsRelink + keyMigrationFailed }
    var needsAttention: Bool { total > 0 }

    /// 「3 条待迁移 · 1 条引用异常」；没有问题返回 nil，界面不显示"0 条"。
    var text: String? {
        guard needsAttention else { return nil }
        var parts: [String] = []
        if migratable > 0 { parts.append("\(migratable) 条待迁移") }
        let issues = blocked + needsRelink + keyMigrationFailed
        if issues > 0 { parts.append("\(issues) 项需处理") }
        return parts.joined(separator: " · ")
    }

    var tone: SettingsTone {
        blocked > 0 || keyMigrationFailed > 0 ? .error : (needsAttention ? .warning : .neutral)
    }
}

/// 账号助手的五种展示状态。
///
/// 旧界面把"未启动"和"协议不匹配"都塞进一条 failed 文案，用户看不出该做什么。
enum SettingsWorkerPhase: Equatable {
    case stopped
    case starting
    case running(String)
    case incompatible(String)
    case failed(String)

    /// 右侧只放结果，不把 health 摘要塞进一行。正常状态弱化。
    var valueText: String {
        switch self {
        case .stopped: return "未运行"
        case .starting: return "启动中"
        case .running: return "运行中"
        case .incompatible: return "版本不匹配"
        case .failed: return "启动失败"
        }
    }

    var title: String {
        switch self {
        case .running(let summary): return summary
        default: return valueText
        }
    }

    var detail: String? {
        switch self {
        case .running(let summary): return summary
        case .incompatible(let detail), .failed(let detail): return detail
        default: return nil
        }
    }

    var tone: SettingsTone {
        switch self {
        case .running, .starting, .stopped: return .neutral
        case .incompatible: return .warning
        case .failed: return .error
        }
    }

    var isRunning: Bool {
        if case .running = self { return true }
        return false
    }
}

/// 只读配置的三态：没读过、正在读、读到了、读取失败。
///
/// 旧界面用 `nil` 同时表示"未知"和"没有"：例如 `credentialStorage == nil` 时
/// 显示"未获取"，但轮换时区读取失败时只留一句红字，界面其余部分仍像已知。
enum SettingsLoadState<Value: Equatable>: Equatable {
    case unknown
    case loading
    case loaded(Value)
    case failed(String)

    var value: Value? {
        if case .loaded(let value) = self { return value }
        return nil
    }

    var failure: String? {
        if case .failed(let message) = self { return message }
        return nil
    }

    var isLoading: Bool {
        if case .loading = self { return true }
        return false
    }
}

// MARK: - 动作决策

/// 设置页可能产生的副作用。视图只执行，不自行判断该做哪几件。
enum SettingsEffect: Equatable {
    case clearPairingCode
    case startBridgeMonitoring
    case stopBridgeMonitoring
    case stopBridgeService
    case disableLaunchAtLogin
    case ensureBridgeTLS
    case restartBridge
    /// 读一次 Widget 服务商 → 发布设置 → 刷新时间线。
    /// 与旧 `reloadWidgets()` 的三步一一对应，不能拆开执行。
    case reloadWidgetDisplay
    case resetConnectionProbe
}

/// 会触发副作用的设置变化。
///
/// **导航不在其中**：切换分类没有任何对应 trigger，所以结构上不可能因为
/// 重建视图而启停服务、生成配对码或重置证书。
enum SettingsTrigger: Equatable {
    case bridgeEnabled(Bool)
    case bridgeLAN(Bool, bridgeEnabled: Bool)
    case magpiePort(bridgeEnabled: Bool)
    case widgetDisplay
}

enum SettingsActionPlan {
    static func effects(for trigger: SettingsTrigger) -> [SettingsEffect] {
        switch trigger {
        case .bridgeEnabled(let enabled):
            // 与旧 `onChange(of: settings.bridgeEnabled)` 逐行等价。
            return enabled
                ? [.clearPairingCode, .startBridgeMonitoring]
                : [.clearPairingCode, .stopBridgeMonitoring, .stopBridgeService, .disableLaunchAtLogin]

        case .bridgeLAN(let enabled, let bridgeEnabled):
            // 改监听范围必须真的重启进程：只改开关不动绑定，等于界面说"已关闭"
            // 而端口还开着。
            guard bridgeEnabled else { return [.clearPairingCode] }
            var effects: [SettingsEffect] = [.clearPairingCode]
            // 先有证书再有监听：桥接不会在没有证书时退回明文。
            if enabled { effects.append(.ensureBridgeTLS) }
            effects.append(.restartBridge)
            return effects

        case .magpiePort(let bridgeEnabled):
            var effects: [SettingsEffect] = [
                .reloadWidgetDisplay,
                .resetConnectionProbe,
                .clearPairingCode,
            ]
            if bridgeEnabled { effects.append(.restartBridge) }
            return effects

        case .widgetDisplay:
            return [.reloadWidgetDisplay]
        }
    }
}

/// 重置证书身份的流程。**没有经过 `confirm()` 就绝不会执行 `tls.reset`。**
enum SettingsTLSResetFlow: Equatable {
    case idle
    case confirming
    case running
    case failed(String)
    /// 证书已经换掉，但端口上还跑着旧证书 —— 不能提示成功。
    case replacedButRestartFailed

    var isBusy: Bool { self == .running }
    var isConfirming: Bool { self == .confirming }

    /// 点「重置证书身份」：只进入确认，不执行。
    mutating func request() -> Bool {
        guard self != .running, self != .confirming else { return false }
        self = .confirming
        return true
    }

    /// 确认框里点「重置」：这时才允许执行。
    mutating func confirm() -> Bool {
        guard self == .confirming else { return false }
        self = .running
        return true
    }

    mutating func cancel() {
        guard self == .confirming else { return }
        self = .idle
    }

    /// 执行结束后由调用方给出两个独立事实，界面不合并成一句"完成"。
    mutating func finish(identityReplaced: Bool, restartSucceeded: Bool, error: String?) {
        if !identityReplaced {
            self = .failed(error ?? "证书身份没有替换成功")
        } else if !restartSucceeded {
            self = .replacedButRestartFailed
        } else {
            self = .idle
        }
    }

    /// 证书已换、但运行中的服务还在用旧证书时，页面必须改口。
    ///
    /// 只有这一种情况会覆盖"HTTPS 服务"的值：`.failed` 表示身份没换成功，
    /// 服务状态本身是真实的，错误交给 `notice` 显示，不能篡改服务状态。
    var certificateStatus: SettingsCertificateStatus? {
        guard self == .replacedButRestartFailed else { return nil }
        return SettingsCertificateStatus(
            value: "运行中 · 证书未生效",
            tone: .warning,
            fingerprintNotServing: "这段指纹属于新证书，端口上仍在用旧证书；重启 AICC 后才会生效。"
        )
    }

    var notice: (text: String, tone: SettingsTone)? {
        switch self {
        case .idle, .confirming, .running:
            return nil
        case .failed(let message):
            return ("证书身份没有替换成功：\(message)", .error)
        case .replacedButRestartFailed:
            return ("证书身份已替换，但桥接没有重启成功：端口上仍是旧证书，请手动重启 AICC。", .warning)
        }
    }
}

/// 证书在页面上的呈现。`fingerprintNotServing` 非空时，指纹属于尚未生效的新证书。
struct SettingsCertificateStatus: Equatable {
    var value: String
    var tone: SettingsTone
    var fingerprintNotServing: String?
}

/// 凭据存储开关的结果。
///
/// 助手没保存成功时，界面必须保留旧的存储方式并显示原因，不能"看起来切过去了"。
enum SettingsCredentialSwitchOutcome: Equatable {
    case applied(Bool)
    case rejected(String)

    static func resolve(saved: Bool, desired: Bool, error: String?) -> SettingsCredentialSwitchOutcome {
        saved ? .applied(desired) : .rejected(error ?? "设置没有保存成功")
    }

    var showsSuccess: Bool {
        if case .applied = self { return true }
        return false
    }

    var failure: String? {
        if case .rejected(let message) = self { return message }
        return nil
    }
}

/// 首选授权的乐观更新。
///
/// 旧代码先改 UI 再发请求，但助手未运行时直接 return，UI 会一直停在用户选的
/// 那一项，而服务端仍是旧值。这里把"该显示什么"与"是否回滚"放在一处。
enum SettingsPreferredAuthOutcome: Equatable {
    case saved(String)
    case rolledBack(String, error: String)

    static func resolve(previous: String, requested: String, saved: Bool, error: String?) -> SettingsPreferredAuthOutcome {
        saved ? .saved(requested) : .rolledBack(previous, error: error ?? "设置没有保存")
    }

    var value: String {
        switch self {
        case .saved(let value): return value
        case .rolledBack(let value, _): return value
        }
    }

    var error: String? {
        if case .rolledBack(_, let error) = self { return error }
        return nil
    }
}

// MARK: - 导航与窗口

/// 分类选择。切换是纯赋值：这里没有任何通道能触发 `SettingsEffect`。
///
/// 不加 `@MainActor`：只被视图在主线程调用，且需要能在 `@StateObject` 的
/// 默认参数里构造。所有状态仍然只在主线程读写。
final class SettingsNavigation: ObservableObject {
    @Published private(set) var category: SettingsCategory
    private let store: SettingsCategoryStore

    init(category: SettingsCategory? = nil, store: SettingsCategoryStore = SettingsCategoryStore()) {
        self.store = store
        self.category = category ?? store.load() ?? .general
    }

    func select(_ category: SettingsCategory) {
        guard category != self.category else { return }
        self.category = category
        store.save(category)
    }
}

/// 记住上次看的是哪一分类：关窗再打开不会跳回第一页。
struct SettingsCategoryStore {
    static let defaultsKey = "AICCSettingsCategory"
    var defaults: UserDefaults = .standard

    func load() -> SettingsCategory? {
        guard let raw = defaults.string(forKey: Self.defaultsKey) else { return nil }
        return SettingsCategory(rawValue: raw)
    }

    func save(_ category: SettingsCategory) {
        defaults.set(category.rawValue, forKey: Self.defaultsKey)
    }
}

/// 窗口尺寸契约。视图与 coordinator 都读这里，避免两处魔法数字。
enum SettingsWindowMetrics {
    static let defaultWidth: CGFloat = 760
    static let defaultHeight: CGFloat = 620
    static let minWidth: CGFloat = 560
    static let minHeight: CGFloat = 520
    static let wideWidth: CGFloat = 960
    static let wideHeight: CGFloat = 720

    /// 低于这个宽度就把左导航收成顶部选择器。
    static let compactBreakpoint: CGFloat = 700
    static let navigationWidth: CGFloat = 152
    static let contentPadding: CGFloat = 24
    static let contentMaxWidth: CGFloat = 640
}

/// 操作结论的摘要。备份路径这类长内容进详情，首屏只留一句结论。
enum SettingsNoticeText {
    static func brief(_ text: String, limit: Int = 44) -> String {
        let firstSentence = text.split(separator: "。", maxSplits: 1).first.map(String.init) ?? text
        guard firstSentence.count > limit else { return firstSentence }
        return String(firstSentence.prefix(limit)) + "…"
    }
}

/// 一次性只读加载的闸门：分类切换不会让 `.task` 重跑。
struct SettingsLoadGate {
    private var ran = false

    mutating func shouldRun() -> Bool {
        guard !ran else { return false }
        ran = true
        return true
    }

    mutating func reset() {
        ran = false
    }
}
