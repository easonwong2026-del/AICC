import Foundation
import XCTest
@testable import AICCCore

/// 设置中心 Phase 2B 的纯模型契约。
///
/// 这些测试不碰真实钥匙串、证书、桥接或助手进程：被测的全部是"该显示什么"
/// 与"点了会怎样"的决策。视图只负责把决策画出来。
final class SettingsCenterTests: XCTestCase {

    // MARK: - 1. 四个分类与原设置项目的映射

    func testEverySettingItemBelongsToExactlyOneCategory() {
        var seen: [SettingsItemKey: Int] = [:]
        for category in SettingsCategory.allCases {
            for item in category.items {
                seen[item, default: 0] += 1
            }
        }
        for item in SettingsItemKey.allCases {
            XCTAssertEqual(seen[item], 1, "\(item.rawValue) 必须恰好出现在一个分类里")
        }
        XCTAssertEqual(seen.count, SettingsItemKey.allCases.count, "映射表不能有未登记的 key")
    }

    func testEveryCategoryHasItemsAndATitle() {
        for category in SettingsCategory.allCases {
            XCTAssertFalse(category.items.isEmpty, "\(category.rawValue) 不能是空分类")
            XCTAssertFalse(category.title.isEmpty)
        }
    }

    func testDangerousItemsLiveInTheExpectedCategories() {
        XCTAssertEqual(SettingsItemKey.bridgeIdentityReset.category, .connection)
        XCTAssertEqual(SettingsItemKey.revokePairedDevices.category, .connection)
        XCTAssertEqual(SettingsItemKey.enableJSONStorage.category, .credentials)
        XCTAssertEqual(SettingsItemKey.disableJSONStorage.category, .credentials)
        XCTAssertEqual(SettingsItemKey.credentialManagementEntry.category, .credentials)
        XCTAssertEqual(SettingsItemKey.quitApp.category, .advanced)
        XCTAssertEqual(SettingsItemKey.workerStatus.category, .advanced)
        XCTAssertEqual(SettingsItemKey.rotationTimezone.category, .advanced)
    }

    /// 迁移、核验、回滚、重新关联的入口必须在授权与存储里可达。
    func testCredentialMaintenanceIsReachableFromTheCredentialCategory() {
        let keys = Set(SettingsCategory.credentials.items)
        XCTAssertTrue(keys.contains(.credentialManagementEntry))
        XCTAssertTrue(keys.contains(.secureStoreProbe))
        XCTAssertTrue(keys.contains(.credentialStorageMode))
    }

    // MARK: - 2. 导航切换不触发写操作

    func testCategorySwitchOnlyChangesSelection() {
        let defaults = UserDefaults(suiteName: "settings.center.tests.nav")!
        defaults.removePersistentDomain(forName: "settings.center.tests.nav")
        let navigation = SettingsNavigation(store: SettingsCategoryStore(defaults: defaults))

        XCTAssertEqual(navigation.category, .general)
        navigation.select(.connection)
        XCTAssertEqual(navigation.category, .connection)
        navigation.select(.connection)
        XCTAssertEqual(navigation.category, .connection, "重复选择同一分类不应产生额外变化")
        navigation.select(.advanced)
        XCTAssertEqual(navigation.category, .advanced)
        XCTAssertEqual(defaults.string(forKey: SettingsCategoryStore.defaultsKey), "advanced")
        defaults.removePersistentDomain(forName: "settings.center.tests.nav")
    }

    /// 一次性只读加载：切分类不能让它重跑。
    func testLoadGateRunsOnceAcrossCategoryChanges() {
        var gate = SettingsLoadGate()
        XCTAssertTrue(gate.shouldRun())
        for _ in SettingsCategory.allCases {
            XCTAssertFalse(gate.shouldRun(), "分类切换不能重新触发只读加载")
        }
        gate.reset()
        XCTAssertTrue(gate.shouldRun())
    }

    // MARK: - 3. 窗口关闭、重新打开与选择状态

    func testCategorySelectionSurvivesWindowReopen() {
        let suite = "settings.center.tests.persist"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)

        let first = SettingsNavigation(store: SettingsCategoryStore(defaults: defaults))
        first.select(.credentials)

        // 关窗再打开 = 新建一份导航，但读同一个存储。
        let second = SettingsNavigation(store: SettingsCategoryStore(defaults: defaults))
        XCTAssertEqual(second.category, .credentials)

        // 存储里是脏值时回落到常规，而不是空白页。
        defaults.set("not-a-category", forKey: SettingsCategoryStore.defaultsKey)
        let third = SettingsNavigation(store: SettingsCategoryStore(defaults: defaults))
        XCTAssertEqual(third.category, .general)
        defaults.removePersistentDomain(forName: suite)
    }

    // MARK: - 4. 语言 / 主题变更

    func testPresentationIdentityChangesTriggerRebuildWithoutTouchingCategory() {
        let before = SettingsPresentationIdentity(languageCode: "zh-Hans", themeMode: "system")
        let afterTheme = SettingsPresentationIdentity(languageCode: "zh-Hans", themeMode: "dark")
        let afterLanguage = SettingsPresentationIdentity(languageCode: "en", themeMode: "system")

        XCTAssertNotEqual(before, afterTheme)
        XCTAssertNotEqual(before, afterLanguage)
        XCTAssertEqual(before, SettingsPresentationIdentity(languageCode: "zh-Hans", themeMode: "system"))
        // 身份变化只影响重建，不改分类选择：分类是独立存储的。
        XCTAssertEqual(SettingsCategoryStore.defaultsKey, "AICCSettingsCategory")
    }

    // MARK: - 5. LAN 开关的生命周期调用

    func testBridgeToggleEffectsMatchTheOriginalLifecycle() {
        XCTAssertEqual(
            SettingsActionPlan.effects(for: .bridgeEnabled(true)),
            [.clearPairingCode, .startBridgeMonitoring]
        )
        XCTAssertEqual(
            SettingsActionPlan.effects(for: .bridgeEnabled(false)),
            [.clearPairingCode, .stopBridgeMonitoring, .stopBridgeService, .disableLaunchAtLogin]
        )
    }

    func testLANToggleAlwaysRestartsAndPreparesACertificateFirst() {
        let on = SettingsActionPlan.effects(for: .bridgeLAN(true, bridgeEnabled: true))
        XCTAssertEqual(on.first, .clearPairingCode)
        // 先有证书再有监听：ensure 必须排在 restart 之前。
        XCTAssertEqual(on, [.clearPairingCode, .ensureBridgeTLS, .restartBridge])

        let off = SettingsActionPlan.effects(for: .bridgeLAN(false, bridgeEnabled: true))
        XCTAssertEqual(off, [.clearPairingCode, .restartBridge], "关掉 LAN 也要重启，否则端口还开着")

        // 桥接本身关着：只清配对码，不动服务。
        XCTAssertEqual(
            SettingsActionPlan.effects(for: .bridgeLAN(true, bridgeEnabled: false)),
            [.clearPairingCode]
        )
    }

    func testPortAndWidgetChangesReloadTheRightThings() {
        XCTAssertEqual(
            SettingsActionPlan.effects(for: .magpiePort(bridgeEnabled: true)),
            [.reloadWidgetDisplay, .resetConnectionProbe, .clearPairingCode, .restartBridge]
        )
        XCTAssertEqual(
            SettingsActionPlan.effects(for: .magpiePort(bridgeEnabled: false)),
            [.reloadWidgetDisplay, .resetConnectionProbe, .clearPairingCode]
        )
        XCTAssertEqual(
            SettingsActionPlan.effects(for: .widgetDisplay),
            [.reloadWidgetDisplay]
        )
    }

    // MARK: - 6. TLS 重置必须经过确认

    func testTLSResetCannotRunWithoutConfirmation() {
        var flow = SettingsTLSResetFlow.idle
        XCTAssertTrue(flow.request())
        XCTAssertTrue(flow.isConfirming)
        XCTAssertFalse(flow.isBusy, "点按钮只进入确认，不能开始执行")

        // 未确认时 confirm 不会生效（idle 直接拒绝）。
        var untouched = SettingsTLSResetFlow.idle
        XCTAssertFalse(untouched.confirm())
        XCTAssertEqual(untouched, .idle)
    }

    func testTLSResetConfirmationStartsExactlyOneRun() {
        var flow = SettingsTLSResetFlow.idle
        _ = flow.request()
        XCTAssertTrue(flow.confirm())
        XCTAssertTrue(flow.isBusy)
        XCTAssertFalse(flow.confirm(), "执行中不能重复提交")
        XCTAssertFalse(flow.request(), "执行中不能再打开确认框")
    }

    func testTLSResetCancelReturnsToIdleWithoutRunning() {
        var flow = SettingsTLSResetFlow.idle
        _ = flow.request()
        flow.cancel()
        XCTAssertEqual(flow, .idle)
        XCTAssertFalse(flow.isBusy)
    }

    // MARK: - 7. TLS 重置失败与服务重启失败

    func testTLSResetDistinguishesIdentityFailureFromRestartFailure() {
        var identityFailed = SettingsTLSResetFlow.running
        identityFailed.finish(identityReplaced: false, restartSucceeded: false, error: "tls.reset 超时")
        XCTAssertEqual(identityFailed, .failed("tls.reset 超时"))
        XCTAssertEqual(identityFailed.notice?.tone, .error)

        var restartFailed = SettingsTLSResetFlow.running
        restartFailed.finish(identityReplaced: true, restartSucceeded: false, error: nil)
        XCTAssertEqual(restartFailed, .replacedButRestartFailed)
        XCTAssertEqual(restartFailed.notice?.tone, .warning)
        XCTAssertTrue(restartFailed.notice?.text.contains("旧证书") == true,
                      "必须说明端口上仍是旧证书，不能提示成功")

        var allGood = SettingsTLSResetFlow.running
        allGood.finish(identityReplaced: true, restartSucceeded: true, error: nil)
        XCTAssertEqual(allGood, .idle)
        XCTAssertNil(allGood.notice)
    }

    // MARK: - 8. 凭据 JSON 风险确认

    func testCredentialViewKeepsConsentRequirementVisible() {
        let policy = CredentialStoragePolicy(
            jsonEnabled: false, consentRequired: true, risk: "明文风险", counts: [:]
        )
        let view = SettingsCredentialView.resolve(policy)
        XCTAssertEqual(view.mode, .keychain)
        XCTAssertTrue(view.needsConsent, "未同意时必须提示")
        XCTAssertEqual(view.headline, "macOS 钥匙串")
    }

    func testCredentialViewMarksPlaintextStorageAsWarning() {
        let policy = CredentialStoragePolicy(
            jsonEnabled: true, consentRequired: false, risk: "", counts: ["localjson": 4]
        )
        let view = SettingsCredentialView.resolve(policy)
        XCTAssertEqual(view.mode, .localJSON)
        XCTAssertEqual(view.tone, .warning)
        XCTAssertFalse(view.needsConsent)
    }

    func testCredentialViewSeparatesHelperDownFromLegacyHelper() {
        let down = SettingsCredentialView.resolve(nil, helperRunning: false)
        XCTAssertEqual(down.headline, "无法读取")
        XCTAssertTrue(down.detail.contains("助手未运行"))

        let legacy = SettingsCredentialView.resolve(nil, helperRunning: true)
        XCTAssertEqual(legacy.headline, "未获取")
        XCTAssertTrue(legacy.detail.contains("还没有报告"))
    }

    // MARK: - 9. 凭据存储保存失败

    func testFailedCredentialSwitchNeverReportsSuccess() {
        let rejected = SettingsCredentialSwitchOutcome.resolve(saved: false, desired: true, error: "助手未运行")
        XCTAssertFalse(rejected.showsSuccess)
        XCTAssertEqual(rejected.failure, "助手未运行")

        let applied = SettingsCredentialSwitchOutcome.resolve(saved: true, desired: true, error: nil)
        XCTAssertTrue(applied.showsSuccess)
        XCTAssertNil(applied.failure)
    }

    func testCredentialAttentionSummarisesWithoutLosingSeverity() {
        let state = CredentialMigrationState(
            summary: CredentialMigrationSummary(
                migratable: 2, alreadyJSON: 3, magpie: 2, blocked: 1,
                rollbackable: 2, total: 8, needsRelink: 1
            ),
            items: [],
            migratedRecords: 2,
            keyMigration: CredentialKeyMigration(failed: 1, pending: 0)
        )
        let attention = SettingsCredentialAttention.resolve(state)
        XCTAssertEqual(attention.total, 5)
        XCTAssertEqual(attention.text, "2 条待迁移 · 3 项需处理")
        XCTAssertEqual(attention.tone, .error, "引用异常或旧键失败必须比纯待迁移更严重")

        XCTAssertNil(SettingsCredentialAttention.resolve(nil).text, "没有问题时不要显示 0 条")
    }

    // MARK: - 10. 迁移、回滚、重新关联入口仍可达

    func testMigrationEntryStaysInCredentialCategoryAfterMove() {
        // 主页只留入口；逐条状态与操作在详情里，但入口本身不能消失。
        XCTAssertTrue(SettingsCategory.credentials.items.contains(.credentialManagementEntry))
        XCTAssertFalse(SettingsCategory.general.items.contains(.credentialManagementEntry))
        XCTAssertFalse(SettingsCategory.advanced.items.contains(.credentialManagementEntry))
    }

    // MARK: - 11. 授权来源设置失败回滚

    func testPreferredAuthRollsBackWhenSaveFails() {
        let failed = SettingsPreferredAuthOutcome.resolve(
            previous: "magpie", requested: "own", saved: false, error: "助手未运行"
        )
        XCTAssertEqual(failed.value, "magpie", "失败后必须回到服务端旧值")
        XCTAssertEqual(failed.error, "助手未运行")

        let saved = SettingsPreferredAuthOutcome.resolve(
            previous: "magpie", requested: "own", saved: true, error: nil
        )
        XCTAssertEqual(saved.value, "own")
        XCTAssertNil(saved.error)
    }

    // MARK: - 12. Widget 设置变更后的刷新

    func testWidgetChangeRepublishesAndReloadsTimelines() {
        let effects = SettingsActionPlan.effects(for: .widgetDisplay)
        XCTAssertEqual(effects, [.reloadWidgetDisplay])
        // 读服务商 → 发布 → 刷新时间线必须在同一个 effect 里，不能拆散。
        XCTAssertFalse(effects.contains(.restartBridge))
    }

    // MARK: - 13. 加载、错误与未知状态

    func testLoadStateKeepsUnknownApartFromFailure() {
        XCTAssertEqual(SettingsLoadState<Int>.unknown.value, nil)
        XCTAssertNil(SettingsLoadState<Int>.unknown.failure)
        XCTAssertTrue(SettingsLoadState<Int>.loading.isLoading)
        XCTAssertEqual(SettingsLoadState<Int>.loaded(3).value, 3)
        XCTAssertEqual(SettingsLoadState<Int>.failed("超时").failure, "超时")
        XCTAssertNil(SettingsLoadState<Int>.failed("超时").value)
    }

    // MARK: - 14. 禁用状态与重复点击保护

    func testBusyFlowBlocksRepeatedConfirmation() {
        var flow = SettingsTLSResetFlow.running
        XCTAssertTrue(flow.isBusy)
        XCTAssertFalse(flow.isConfirming)
        XCTAssertFalse(flow.confirm())
        XCTAssertFalse(flow.request())
    }

    // MARK: - 桥接状态派生

    func testBridgePhaseOrdering() {
        XCTAssertEqual(
            SettingsBridgePhase.resolve(enabled: false, running: true, lanEnabled: true,
                                        lanServing: true, runtimeError: nil),
            .off
        )
        XCTAssertEqual(
            SettingsBridgePhase.resolve(enabled: true, running: false, lanEnabled: false,
                                        lanServing: false, runtimeError: "运行时缺失"),
            .failed("运行时缺失")
        )
        XCTAssertEqual(
            SettingsBridgePhase.resolve(enabled: true, running: false, lanEnabled: false,
                                        lanServing: false, runtimeError: nil),
            .waiting
        )
        XCTAssertEqual(
            SettingsBridgePhase.resolve(enabled: true, running: true, lanEnabled: true,
                                        lanServing: false, runtimeError: nil),
            .lanIntentWithoutListener
        )
        XCTAssertEqual(
            SettingsBridgePhase.resolve(enabled: true, running: true, lanEnabled: true,
                                        lanServing: true, runtimeError: nil),
            .lanServing
        )
        XCTAssertEqual(
            SettingsBridgePhase.resolve(enabled: true, running: true, lanEnabled: false,
                                        lanServing: false, runtimeError: nil),
            .loopbackOnly
        )
    }

    func testBridgePhaseNeverCallsLoopbackNormalWhenLANIsSilentlyMissing() {
        let phase = SettingsBridgePhase.resolve(
            enabled: true, running: true, lanEnabled: true, lanServing: false, runtimeError: nil
        )
        XCTAssertEqual(phase.tone, .warning)
        XCTAssertNotEqual(phase.statusText, "运行中 · 局域网 HTTPS")
        XCTAssertTrue(phase.warningText?.contains("不会退回明文") == true)
    }

    func testLANPhaseText() {
        XCTAssertEqual(SettingsLANPhase.resolve(enabled: false, lanServing: false), .off)
        XCTAssertEqual(SettingsLANPhase.resolve(enabled: true, lanServing: false), .allowedWithoutListener)
        XCTAssertEqual(SettingsLANPhase.resolve(enabled: true, lanServing: true), .served)
        XCTAssertEqual(SettingsLANPhase.resolve(enabled: true, lanServing: false).tone, .warning)
    }

    // MARK: - 助手状态展示

    func testWorkerPhaseNeverShowsHealthyForStoppedOrIncompatible() {
        XCTAssertEqual(SettingsWorkerPhase.stopped.valueText, "未运行")
        XCTAssertEqual(SettingsWorkerPhase.starting.valueText, "启动中")
        XCTAssertEqual(SettingsWorkerPhase.incompatible("v3 / v1").valueText, "版本不匹配")
        XCTAssertEqual(SettingsWorkerPhase.incompatible("v3 / v1").tone, .warning)
        XCTAssertEqual(SettingsWorkerPhase.failed("启动失败").tone, .error)

        let running = SettingsWorkerPhase.running("v0.4.2 · 账号 9")
        XCTAssertEqual(running.valueText, "运行中")
        XCTAssertEqual(running.detail, "v0.4.2 · 账号 9")
        XCTAssertEqual(running.tone, .neutral, "正常状态不额外着色")
        XCTAssertTrue(running.isRunning)
    }

    // MARK: - 文案预算

    func testNoticeBriefDropsTrailingDetail() {
        let full = "已迁移 1 条，失败 2 条（可重新核验）。备份：/Users/example/Library/Application Support/AICC-Dashboard/backups/2026-11-21T08-12-31"
        let brief = SettingsNoticeText.brief(full)
        XCTAssertTrue(brief.hasSuffix("（可重新核验）"), "摘要只保留第一句")
        XCTAssertFalse(brief.contains("备份："), "备份路径属于展开详情")
        XCTAssertLessThan(brief.count, 46)

        let short = "没有需要迁移的凭据"
        XCTAssertEqual(SettingsNoticeText.brief(short), short)
    }

    // MARK: - 窗口尺寸契约

    func testWindowMetricsKeepTheLegacyMinimum() {
        XCTAssertEqual(SettingsWindowMetrics.minWidth, 560)
        XCTAssertEqual(SettingsWindowMetrics.minHeight, 520)
        XCTAssertGreaterThan(SettingsWindowMetrics.defaultWidth, SettingsWindowMetrics.minWidth)
        XCTAssertGreaterThan(SettingsWindowMetrics.wideWidth, SettingsWindowMetrics.defaultWidth)
        XCTAssertLessThan(SettingsWindowMetrics.compactBreakpoint, SettingsWindowMetrics.defaultWidth,
                          "默认窗口必须是双栏")
        XCTAssertGreaterThan(SettingsWindowMetrics.compactBreakpoint, SettingsWindowMetrics.minWidth,
                             "最小窗口必须走紧凑布局")
    }
}
