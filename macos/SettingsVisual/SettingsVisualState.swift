import Foundation

/// 设置页离屏渲染的状态样本。
///
/// 只存在于 `-D AICC_VISUAL_TEST -D AICC_SETTINGS_VISUAL` 的构建里。
/// 所有值都是编的，不含任何真实令牌、指纹或账号。
struct SettingsVisualState {
    var workerState: AccountWorkerManager.State = .stopped
    var preferredAuthSource = "magpie"
    var authSettingsError: String?
    var credentialStorage: CredentialStoragePolicy?
    var secureStoreSummary = "未检查"
    var migration: CredentialMigrationState?
    var migrationNotice: String?
    var verifyReport: CredentialVerifyReport?
    var migrationBusy = false
    var bridgeTls: BridgeTlsStatus?
    var bridgeTlsError: String?

    // 服务器 / 登录项（不进 AccountWorkerManager）
    var bridgeEnabled = false
    var bridgeLANEnabled = false
    var serverRunning = false
    var lanServing = false
    var runtimeError: String?
    var loginAtLaunchEnabled = false
    var loginAtLaunchError: String?

    // 设置页本地状态
    var magpiePort = 3425
    var connection = "未检查"
    var pairingCode = ""
    var securityNotice: String?
    var rotationTimezone = "Asia/Shanghai"
    var rotationTimezones = ["Asia/Shanghai", "UTC"]
    var rotationRule = "错过开始时间 10 分钟以上本轮不补跑；每次切换前预留 5 分钟恢复余量。"
    var rotationError: String?
    var rotationLoaded = true
    var tlsReset: SettingsTLSResetFlow = .idle
}

// MARK: - 健康样本

extension SettingsVisualState {
    static func health(
        protocolVersion: Int = AccountWorkerProtocol.version,
        status: String = "ok",
        databaseOK: Bool = true,
        accountCount: Int = 9,
        jsonStorage: Bool = false,
        consentRequired: Bool = true
    ) -> AccountWorkerHealth {
        AccountWorkerHealth.parse([
            "status": status,
            "protocol": protocolVersion,
            "worker": ["version": "0.4.2", "aiccVersion": "2.13.0"],
            "upstream": ["version": "1.2.7", "commit": "9f3c1d7"],
            "paths": ["dataDir": "/Users/example/Library/Application Support/AICC-Dashboard/accounts",
                      "wbSwitchHomeOwned": true],
            "database": ["ok": databaseOK, "schemaVersion": 23,
                         "counts": ["accounts": accountCount]],
            "providers": ["workbuddy": ["state": "plaintext_token"],
                          "qoder": ["state": "auth_file_present"]],
            "credentials": [
                "jsonStorageEnabled": jsonStorage,
                "consentRequired": consentRequired,
                "risk": "凭据目录是 0700、文件是 0600，但这只能挡住同机的其他用户，挡不住同一个 macOS 用户下运行的其他程序。",
            ],
            "authorizations": [
                "storage": [
                    ["kind": "localjson", "count": jsonStorage ? 4 : 0],
                    ["kind": "keychain", "count": jsonStorage ? 3 : 7],
                    ["kind": "magpie", "count": 2],
                ],
            ],
        ])!
    }

    /// 桥接开启、局域网 HTTPS 正在服务（设备安全详情的正常样本）。
    static var bridgeServing: SettingsVisualState {
        var state = healthy
        state.bridgeEnabled = true
        state.bridgeLANEnabled = true
        state.serverRunning = true
        state.lanServing = true
        return state
    }

    /// 正常：助手在跑、Magpie 已连接、钥匙串存储。
    static var healthy: SettingsVisualState {
        var state = SettingsVisualState()
        state.workerState = .running(health())
        state.credentialStorage = CredentialStoragePolicy(
            jsonEnabled: false,
            consentRequired: true,
            risk: "",
            counts: [CredentialStoragePolicy.kindKeychain: 7,
                     CredentialStoragePolicy.kindMagpie: 2]
        )
        state.secureStoreSummary = "可用（keychain，12 ms）"
        state.migration = migrationNeeded
        state.verifyReport = CredentialVerifyReport(
            ok: 5, broken: 0, pendingKeychain: 2, orphanCount: 0,
            brokenFiles: 0, pendingCleanup: 0, needsRelink: 1
        )
        state.bridgeTls = BridgeTlsStatus(
            spki: "a7f3c19e4b62d805f1c3a97b6e2d5480c9f7ab31d64e028b5c1f9a7e3d806b24",
            notAfter: "2027-11-20",
            generation: 2,
            caCreated: true,
            leafReissued: false
        )
        state.connection = "已连接 · Codex、Claude、WorkBuddy · 4 张额度卡"
        state.migrationNotice = "已迁移 1 条，失败 2 条（可重新核验）。备份：/Users/example/Library/Application Support/AICC-Dashboard/backups/2026-11-21T08-12-31"
        return state
    }

    static var migrationNeeded: CredentialMigrationState {
        CredentialMigrationState(
            summary: CredentialMigrationSummary(
                migratable: 2, alreadyJSON: 3, magpie: 2, blocked: 1,
                rollbackable: 2, total: 8, needsRelink: 1
            ),
            items: [
                CredentialMigrationItem(
                    domain: "workbuddy", platform: "workbuddy",
                    accountId: "acc-wb-1", nickname: "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
                    source: "own", storage: "keychain",
                    reference: "keychain:workbuddy/acc-wb-1/own/access",
                    migratable: true, rollbackable: true, blocked: nil
                ),
                CredentialMigrationItem(
                    domain: "workbuddy", platform: "workbuddy",
                    accountId: "acc-wb-2", nickname: "设计外包",
                    source: "own", storage: "keychain",
                    reference: "keychain:workbuddy/acc-wb-2/own/access",
                    migratable: true, rollbackable: false, blocked: nil
                ),
                CredentialMigrationItem(
                    domain: "qoder", platform: "qoder_cn",
                    accountId: "qsa-cn-1", nickname: "Qoder CN 主账号",
                    source: "own_browser", storage: "localjson",
                    reference: "localjson:v1:0b9f2a1c",
                    migratable: false, rollbackable: true, blocked: "already_json"
                ),
                CredentialMigrationItem(
                    domain: "workbuddy", platform: "workbuddy",
                    accountId: "acc-mag-1", nickname: "Magpie 导入账号",
                    source: "magpie", storage: "keychain",
                    reference: "keychain:workbuddy/acc-mag-1/magpie",
                    migratable: false, rollbackable: false, blocked: "needs_relink",
                    action: "relink"
                ),
                CredentialMigrationItem(
                    domain: "workbuddy", platform: "workbuddy",
                    accountId: "acc-bulk-9", nickname: "批量账号 9",
                    source: "own", storage: "keychain",
                    reference: nil,
                    migratable: false, rollbackable: false, blocked: "reference_unusable"
                ),
            ],
            migratedRecords: 2,
            keyMigration: CredentialKeyMigration(failed: 1, pending: 0)
        )
    }
}

// MARK: - 场景

enum SettingsVisualScenes {
    struct Scene {
        let name: String
        let category: SettingsCategory
        let size: CGSize
        let scheme: ColorSchemeKind
        let state: SettingsVisualState
        let mutate: (SettingsModel) -> Void

        init(
            name: String,
            category: SettingsCategory,
            size: CGSize,
            scheme: ColorSchemeKind,
            state: SettingsVisualState,
            mutate: @escaping (SettingsModel) -> Void = { _ in }
        ) {
            self.name = name
            self.category = category
            self.size = size
            self.scheme = scheme
            self.state = state
            self.mutate = mutate
        }
    }

    enum ColorSchemeKind: String {
        case light
        case dark
    }

    static let compact = CGSize(width: 560, height: 520)
    static let regular = CGSize(width: 760, height: 620)
    static let wide = CGSize(width: 960, height: 720)

    /// 24 张基础场景：4 分类 × 3 尺寸 × 浅深色。
    static var basics: [Scene] {
        var scenes: [Scene] = []
        for size in [compact, regular, wide] {
            for scheme in [ColorSchemeKind.light, .dark] {
                for category in SettingsCategory.allCases {
                    let side = "\(Int(size.width))x\(Int(size.height))"
                    scenes.append(Scene(
                        name: "settings-\(category.rawValue)-\(side)-\(scheme.rawValue)",
                        category: category,
                        size: size,
                        scheme: scheme,
                        state: .healthy
                    ))
                }
            }
        }
        return scenes
    }

    /// 关键状态、错误与弹层。
    static var states: [Scene] {
        var scenes: [Scene] = []

        // Magpie 连接失败
        var magpieFailed = SettingsVisualState.healthy
        magpieFailed.connection = "无法连接，请启动 Magpie 并检查端口。"
        scenes.append(Scene(name: "settings-magpie-failed-760x620-light", category: .connection,
                            size: regular, scheme: .light, state: magpieFailed))

        // 桥接：开、LAN 关（仅本机）
        var bridgeLoopback = SettingsVisualState.healthy
        bridgeLoopback.bridgeEnabled = true
        bridgeLoopback.serverRunning = true
        scenes.append(Scene(name: "settings-bridge-loopback-760x620-light", category: .connection,
                            size: regular, scheme: .light, state: bridgeLoopback))

        // 桥接：开、LAN 开、HTTPS 正常
        var bridgeLAN = bridgeLoopback
        bridgeLAN.bridgeLANEnabled = true
        bridgeLAN.lanServing = true
        scenes.append(Scene(name: "settings-bridge-lan-760x620-light", category: .connection,
                            size: regular, scheme: .light, state: bridgeLAN))
        scenes.append(Scene(name: "settings-bridge-lan-760x620-dark", category: .connection,
                            size: regular, scheme: .dark, state: bridgeLAN))

        // 桥接：开了 LAN 但 HTTPS 没起来
        var lanNoListener = bridgeLoopback
        lanNoListener.bridgeLANEnabled = true
        lanNoListener.lanServing = false
        scenes.append(Scene(name: "settings-bridge-lan-no-listener-760x620-light", category: .connection,
                            size: regular, scheme: .light, state: lanNoListener))

        // 运行时缺失
        var runtimeMissing = SettingsVisualState.healthy
        runtimeMissing.bridgeEnabled = true
        runtimeMissing.serverRunning = false
        runtimeMissing.runtimeError = "内置 Python 运行时缺失：App 包不完整，请重新安装。"
        scenes.append(Scene(name: "settings-bridge-runtime-missing-760x620-light", category: .connection,
                            size: regular, scheme: .light, state: runtimeMissing))

        // 已生成配对码
        var pairing = bridgeLoopback
        pairing.pairingCode = "482913"
        pairing.loginAtLaunchEnabled = true
        scenes.append(Scene(name: "settings-bridge-pairing-code-760x620-light", category: .connection,
                            size: regular, scheme: .light, state: pairing))
        scenes.append(Scene(name: "settings-bridge-pairing-code-560x520-light", category: .connection,
                            size: compact, scheme: .light, state: pairing))

        // 登录项失败
        var loginFailed = bridgeLoopback
        loginFailed.loginAtLaunchError = "操作无法完成。（com.apple.security.application-groups 错误 1。）"
        scenes.append(Scene(name: "settings-bridge-login-error-760x620-light", category: .connection,
                            size: regular, scheme: .light, state: loginFailed))

        // 证书身份重置：替换失败
        var resetFailed = bridgeLoopback
        resetFailed.tlsReset = .failed("tls.reset 超时：助手 30 秒内没有返回")
        scenes.append(Scene(name: "settings-bridge-reset-failed-960x720-light", category: .connection,
                            size: wide, scheme: .light, state: resetFailed))

        // 证书已换但服务未重启
        var restartFailed = bridgeLoopback
        restartFailed.tlsReset = .replacedButRestartFailed
        scenes.append(Scene(name: "settings-bridge-restart-failed-960x720-light", category: .connection,
                            size: wide, scheme: .light, state: restartFailed))

        // 证书未准备 + 指纹复制提示
        var noCert = SettingsVisualState.healthy
        noCert.bridgeTls = nil
        noCert.bridgeEnabled = true
        noCert.serverRunning = true
        scenes.append(Scene(name: "settings-bridge-no-certificate-960x720-light", category: .connection,
                            size: wide, scheme: .light, state: noCert))

        // 凭据：本地 JSON
        var json = SettingsVisualState.healthy
        json.credentialStorage = CredentialStoragePolicy(
            jsonEnabled: true,
            consentRequired: false,
            risk: "",
            counts: [CredentialStoragePolicy.kindJSON: 4,
                     CredentialStoragePolicy.kindKeychain: 3,
                     CredentialStoragePolicy.kindMagpie: 2]
        )
        scenes.append(Scene(name: "settings-credentials-json-760x620-light", category: .credentials,
                            size: regular, scheme: .light, state: json))
        scenes.append(Scene(name: "settings-credentials-json-760x620-dark", category: .credentials,
                            size: regular, scheme: .dark, state: json))

        // 凭据：需要迁移 + 尚未核验（与基础样本的数字不同，避免两张一样的截图）
        var pendingMigration = SettingsVisualState.healthy
        pendingMigration.migration = CredentialMigrationState(
            summary: CredentialMigrationSummary(
                migratable: 5, alreadyJSON: 1, magpie: 1, blocked: 0,
                rollbackable: 0, total: 7, needsRelink: 2
            ),
            items: (SettingsVisualState.migrationNeeded.items),
            migratedRecords: 0,
            keyMigration: .empty
        )
        pendingMigration.verifyReport = nil
        scenes.append(Scene(name: "settings-credentials-migration-needed-760x620-light", category: .credentials,
                            size: regular, scheme: .light, state: pendingMigration))


        // 首选授权保存失败（助手未运行 + 长错误）
        var authFailed = SettingsVisualState.healthy
        authFailed.authSettingsError = "助手未运行，设置没有保存。上一次成功读取设置是在 08:12，当前显示的是本地默认值。"
        authFailed.workerState = .stopped
        scenes.append(Scene(name: "settings-credentials-auth-error-760x620-light", category: .credentials,
                            size: regular, scheme: .light, state: authFailed))

        // 禁用态：助手未运行 → 不能启用本地 JSON
        var disabled = SettingsVisualState.healthy
        disabled.workerState = .stopped
        disabled.credentialStorage = nil
        disabled.secureStoreSummary = "助手未运行"
        scenes.append(Scene(name: "settings-credentials-disabled-560x520-light", category: .credentials,
                            size: compact, scheme: .light, state: disabled))

        // 高级：助手未运行
        var workerDown = SettingsVisualState.healthy
        workerDown.workerState = .failed("助手未启动\n找不到 Contents/Helpers/aicc-account-worker")
        workerDown.rotationLoaded = true
        workerDown.rotationTimezones = []
        workerDown.rotationTimezone = ""
        workerDown.rotationRule = ""
        workerDown.rotationError = "读不到轮换设置：账号助手未运行"
        scenes.append(Scene(name: "settings-advanced-worker-down-760x620-dark", category: .advanced,
                            size: regular, scheme: .dark, state: workerDown))

        // 高级：协议不匹配
        var incompatible = SettingsVisualState.healthy
        incompatible.workerState = .running(SettingsVisualState.health(protocolVersion: 3))
        scenes.append(Scene(name: "settings-advanced-incompatible-760x620-light", category: .advanced,
                            size: regular, scheme: .light, state: incompatible))

        // 高级：轮换时区读取失败
        var rotationFailed = SettingsVisualState.healthy
        rotationFailed.rotationError = "读不到轮换设置：助手未运行"
        rotationFailed.rotationLoaded = true
        rotationFailed.rotationTimezones = []
        rotationFailed.rotationTimezone = ""
        rotationFailed.rotationRule = ""
        scenes.append(Scene(name: "settings-advanced-rotation-error-760x620-light", category: .advanced,
                            size: regular, scheme: .light, state: rotationFailed))

        return scenes
    }

    /// 独立详情与弹层。
    static var sheets: [Scene] {
        var scenes: [Scene] = []

        scenes.append(Scene(name: "settings-credential-detail-620x620-light", category: .credentials,
                            size: CGSize(width: 620, height: 620), scheme: .light, state: .healthy))
        scenes.append(Scene(name: "settings-credential-detail-620x620-dark", category: .credentials,
                            size: CGSize(width: 620, height: 620), scheme: .dark, state: .healthy))

        var emptyInventory = SettingsVisualState.healthy
        emptyInventory.migration = nil
        emptyInventory.verifyReport = nil
        scenes.append(Scene(name: "settings-credential-detail-empty-620x620-light", category: .credentials,
                            size: CGSize(width: 620, height: 620), scheme: .light, state: emptyInventory))

        return scenes
    }
}
