import XCTest
@testable import AICCCore

/// 自动计划中心与轮换配置的纯模型契约。
///
/// 这些测试不碰真实账号、不关客户端、不发写请求：被测的是"该显示什么"
/// 与"保存/启用的顺序"，视图只负责画出来。
final class AutomationCenterTests: XCTestCase {

    // MARK: - 五类自动计划的状态派生

    func testUnknownIsNotShownAsOff() {
        // 什么都没读到：不能显示"未开启"，必须显示"未知"。
        let rows = AutomationPlanBuilder.rows(AutomationInputs())
        for row in rows {
            XCTAssertEqual(row.state, .unknown, "\(row.kind.rawValue) 未读取时必须是未知")
            XCTAssertNotEqual(row.statusText, "未开启")
            XCTAssertNotEqual(row.summary, "未开启")
        }
    }

    func testLoadingIsDistinctFromUnknown() {
        var inputs = AutomationInputs()
        inputs.checkinLoading = true
        inputs.growthLoading = true
        inputs.travelLoading = true
        inputs.creditsLoading = true
        inputs.rotationLoading = true

        for row in AutomationPlanBuilder.rows(inputs) {
            XCTAssertEqual(row.state, .loading, "\(row.kind.rawValue) 读取中不能被当成未知或关闭")
            XCTAssertEqual(row.statusText, "读取中")
        }
    }

    func testReadFailureIsSurfacedNotSwallowedAsOff() {
        var inputs = AutomationInputs()
        inputs.checkinError = "助手未运行"
        inputs.growthError = "助手未运行"
        inputs.travelError = "助手未运行"
        inputs.creditsError = "助手未运行"
        inputs.rotationError = "助手未运行"

        for row in AutomationPlanBuilder.rows(inputs) {
            guard case .failed(let message) = row.state else {
                return XCTFail("\(row.kind.rawValue) 读取失败必须暴露为 failed")
            }
            XCTAssertEqual(message, "助手未运行")
            XCTAssertEqual(row.tone, .error)
        }
        XCTAssertTrue(inputs.checkinError == "助手未运行")
    }

    func testCheckinCountsEnabledAccounts() {
        var auto = CheckinAutoStatus()
        auto.enabled = true
        auto.enabledAccounts = 5
        auto.totalAccounts = 8
        var inputs = AutomationInputs()
        inputs.checkinAuto = auto

        let row = AutomationPlanBuilder.checkin(inputs, now: Date())
        XCTAssertEqual(row.state, .on)
        XCTAssertEqual(row.summary, "5/8 个账号已开启")
        XCTAssertEqual(row.tone, .ok)
    }

    func testGrowthReportsItsIntervalAndRunningState() {
        var auto = GrowthAutoStatus(json: ["enabled": true, "intervalSeconds": 86_400])
        auto.running = false
        var inputs = AutomationInputs()
        inputs.growthAuto = auto

        let row = AutomationPlanBuilder.growth(inputs, now: Date())
        XCTAssertEqual(row.state, .on)
        XCTAssertTrue(row.summary.contains("每天检查一次"))

        auto.running = true
        inputs.growthAuto = auto
        XCTAssertEqual(AutomationPlanBuilder.growth(inputs, now: Date()).state, .running)
    }

    func testTravelNeedsAtLeastOneEnabledAccount() {
        var inputs = AutomationInputs()
        inputs.travelAuto = TravelAutoStatus(json: [
            "owner": "AICC.app",
            "runsOnlyWhileAppRunning": true,
            "departIntervalSecs": 1800,
            "claimIntervalSecs": 600,
            "enabledAccounts": 0,
            "busy": false,
        ])
        XCTAssertEqual(AutomationPlanBuilder.travel(inputs, now: Date()).state, .off)

        inputs.travelAuto = TravelAutoStatus(json: [
            "owner": "AICC.app",
            "runsOnlyWhileAppRunning": true,
            "departIntervalSecs": 1800,
            "claimIntervalSecs": 600,
            "enabledAccounts": 3,
            "busy": false,
        ])
        let row = AutomationPlanBuilder.travel(inputs, now: Date())
        XCTAssertEqual(row.state, .on)
        XCTAssertEqual(row.summary, "3 个账号已开启")
    }

    func testCreditsOnlyCountsSupportedAccounts() {
        var inputs = AutomationInputs()
        inputs.creditsLoaded = true
        inputs.creditsAuto = [
            QoderCreditsAutoItem(json: ["accountId": "a", "autoEnabled": true, "supported": true])!,
            QoderCreditsAutoItem(json: ["accountId": "b", "autoEnabled": true, "supported": false])!,
            QoderCreditsAutoItem(json: ["accountId": "c", "autoEnabled": false, "supported": true])!,
        ]
        let row = AutomationPlanBuilder.credits(inputs, now: Date())
        XCTAssertEqual(row.state, .on)
        XCTAssertEqual(row.summary, "1/2 个账号已开启", "不支持的账号不能计入已开启")

        inputs.creditsAuto = []
        let empty = AutomationPlanBuilder.credits(inputs, now: Date())
        XCTAssertEqual(empty.state, .off)
        XCTAssertEqual(empty.summary, "暂无支持账号")
    }

    // MARK: - 轮换概览

    func testRotationOverviewSeparatesUnconfiguredOffAndOn() {
        let empty = RotationOverview.make(status: rotationStatus(enabled: false, accountIds: []))
        XCTAssertFalse(empty.configured)
        XCTAssertEqual(empty.scheduleText, "还没有设置自动轮换")
        XCTAssertEqual(empty.compactSummary, "未配置")

        let off = RotationOverview.make(status: rotationStatus(enabled: false, accountIds: ["a", "b"]))
        XCTAssertTrue(off.configured)
        XCTAssertEqual(off.accountCount, 2)
        XCTAssertEqual(off.state, .off)
        XCTAssertTrue(off.scheduleText.contains("周一至周五"))
        XCTAssertEqual(off.compactSummary, "未开启 · 2 个账号")
        XCTAssertNil(off.nextRunText, "关闭的计划没有下一次执行")

        let on = RotationOverview.make(status: rotationStatus(enabled: true, accountIds: ["a", "b"]))
        XCTAssertEqual(on.state, .on)
        XCTAssertEqual(on.compactSummary, "已开启 · 2 个账号")
    }

    func testRotationOverviewNeverMasksRecovery() {
        let recovery = RotationOverview.make(status: rotationRecoveryStatus(enabled: true))
        XCTAssertTrue(recovery.needsRecovery)
        XCTAssertTrue(recovery.planEnabled)
        XCTAssertEqual(recovery.state, .attention("需要恢复"))
        XCTAssertEqual(recovery.state.tone, .warning)
        XCTAssertNotNil(recovery.recoveryText)
        XCTAssertEqual(recovery.compactSummary, "已开启 · 需要恢复")

        // 计划已关闭但上一轮恢复失败：仍然要警告，但不能说计划还在运行。
        let closed = RotationOverview.make(status: rotationRecoveryStatus(enabled: false))
        XCTAssertTrue(closed.needsRecovery)
        XCTAssertFalse(closed.planEnabled)
        XCTAssertEqual(closed.state, .attention("需要恢复"))
        XCTAssertEqual(closed.compactSummary, "已关闭 · 需要恢复")
    }

    func testRotationOverviewSeparatesStoppingFromRunning() {
        // 开关已关、这一轮还在安全收尾：既不是"已开启"，也不是"空闲"。
        let stopping = RotationOverview.make(status: rotationDwellingStatus(enabled: false))
        XCTAssertEqual(stopping.state, .attention("正在停止"))
        XCTAssertEqual(stopping.compactSummary, "已关闭 · 正在停止")

        let running = RotationOverview.make(status: rotationDwellingStatus(enabled: true))
        XCTAssertEqual(running.state, .running)
        XCTAssertEqual(running.compactSummary, "已开启 · 1 个账号")
    }

    func testRotationOverviewKeepsUnknownApartFromOff() {
        let unknown = RotationOverview.make(status: nil)
        XCTAssertEqual(unknown.state, .unknown)
        XCTAssertEqual(unknown.scheduleText, "—")

        let failed = RotationOverview.make(status: nil, error: "助手未运行")
        XCTAssertEqual(failed.state, .failed("助手未运行"))
        XCTAssertNotEqual(failed.state, .off)
    }

    func testRotationRowUsesOverviewState() {
        var inputs = AutomationInputs()
        inputs.rotation = rotationStatus(enabled: true, accountIds: ["a"])
        let row = AutomationPlanBuilder.rotation(inputs, now: Date())
        XCTAssertEqual(row.state, .on)
        XCTAssertEqual(row.summary, "已开启 · 1 个账号")
    }

    // MARK: - 编辑草稿

    func testDraftDetectsEveryFieldChange() {
        let plan = rotationPlan(enabled: true)
        var draft = RotationDraft(plan: plan)
        XCTAssertFalse(draft.isDirty(against: plan))

        draft.weekdays = [1, 3]
        XCTAssertTrue(draft.isDirty(against: plan), "星期变化必须算未保存")

        draft = RotationDraft(plan: plan)
        draft.startMinutes = 21 * 60
        XCTAssertTrue(draft.isDirty(against: plan))

        draft = RotationDraft(plan: plan)
        draft.timezone = "UTC"
        XCTAssertTrue(draft.isDirty(against: plan))

        draft = RotationDraft(plan: plan)
        draft.accountIds = ["a"]
        XCTAssertTrue(draft.isDirty(against: plan))
    }

    func testDraftSelectionAndOrderAreStable() {
        var draft = RotationDraft(plan: rotationPlan(enabled: false))
        draft.accountIds = []
        draft.toggleAccount("a", on: true)
        draft.toggleAccount("b", on: true)
        draft.toggleAccount("c", on: true)
        XCTAssertEqual(draft.accountIds, ["a", "b", "c"])

        draft.moveAccount("c", by: -1)
        XCTAssertEqual(draft.accountIds, ["a", "c", "b"])
        draft.moveAccount("c", by: -1)
        XCTAssertEqual(draft.accountIds, ["c", "a", "b"])
        draft.moveAccount("c", by: -1)
        XCTAssertEqual(draft.accountIds, ["c", "a", "b"], "越界的移动必须是无操作")

        draft.toggleAccount("a", on: false)
        XCTAssertEqual(draft.accountIds, ["c", "b"])
        draft.toggleAccount("c", on: true)
        XCTAssertEqual(draft.accountIds, ["c", "b"], "重复勾选不会重复插入")
    }

    func testDraftValidityAndCrossMidnight() {
        var draft = RotationDraft(plan: rotationPlan(enabled: false))
        draft.accountIds = []
        XCTAssertFalse(draft.isValid, "没有账号不能保存")
        draft.accountIds = ["a"]
        draft.weekdays = []
        XCTAssertFalse(draft.isValid, "没有星期不能保存")
        draft.weekdays = [1]
        XCTAssertTrue(draft.isValid)

        draft.startMinutes = 22 * 60 + 30
        draft.endMinutes = 7 * 60
        XCTAssertTrue(draft.crossesMidnight)
        XCTAssertTrue(draft.windowText.contains("次日"))
    }

    func testDraftParamsUseExistingPlanKeys() {
        var draft = RotationDraft(plan: rotationPlan(enabled: false))
        draft.weekdays = [5, 1]
        draft.accountIds = ["b", "a"]
        draft.timezone = "UTC"
        let params = draft.params

        XCTAssertEqual(Set(params.keys), ["weekdays", "startMinutes", "endMinutes", "accountIds", "dwellSeconds", "timezone"])
        XCTAssertEqual(params["weekdays"] as? [Int], [1, 5], "星期按序提交")
        XCTAssertEqual(params["accountIds"] as? [String], ["b", "a"], "账号顺序原样提交")
        XCTAssertEqual(params["timezone"] as? String, "UTC")
    }

    // MARK: - 保存并开启

    func testSaveFlowOrderIsSaveThenEnableThenReload() {
        XCTAssertEqual(
            RotationSaveFlow.steps(dirty: true, enable: true),
            [.save, .enable, .reload]
        )
        XCTAssertEqual(
            RotationSaveFlow.steps(dirty: false, enable: true),
            [.enable, .reload],
            "没有改动时不需要重复保存"
        )
        XCTAssertEqual(RotationSaveFlow.steps(dirty: true, enable: false), [.save])
        XCTAssertEqual(RotationSaveFlow.steps(dirty: false, enable: false), [])
    }

    func testSaveFailureStopsBeforeEnabling() async {
        var enabled = false
        var reloaded = false
        let result = await RotationSaveFlow.run(
            dirty: true,
            enable: true,
            save: { false },
            setEnabled: { _ in enabled = true; return true },
            reload: { reloaded = true }
        )
        XCTAssertEqual(result, .saveFailed)
        XCTAssertFalse(enabled, "保存失败绝不能继续启用旧计划")
        XCTAssertFalse(reloaded)
    }

    func testEnableFailureReloadsServerState() async {
        var reloaded = false
        let result = await RotationSaveFlow.run(
            dirty: true,
            enable: true,
            save: { true },
            setEnabled: { _ in false },
            reload: { reloaded = true }
        )
        XCTAssertEqual(result, .enableFailed)
        XCTAssertTrue(reloaded, "启用失败必须回读真实状态，界面不能停在已开启")
    }

    func testSuccessfulSaveAndEnableReportsEnabled() async {
        var calls: [String] = []
        let result = await RotationSaveFlow.run(
            dirty: true,
            enable: true,
            save: { calls.append("save"); return true },
            setEnabled: { value in calls.append("enable:\(value)"); return true },
            reload: { calls.append("reload") }
        )
        XCTAssertEqual(result, .enabled)
        XCTAssertEqual(calls, ["save", "enable:true", "reload"])
    }

    func testSaveOnlyDoesNotEnable() async {
        var enabled = false
        let result = await RotationSaveFlow.run(
            dirty: true,
            enable: false,
            save: { true },
            setEnabled: { _ in enabled = true; return true },
            reload: {}
        )
        XCTAssertEqual(result, .savedOnly)
        XCTAssertFalse(enabled, "仅保存不能顺手开启")
    }

    // MARK: - 关闭与停止

    func testDisablingNeverRequiresSavingAnInvalidForm() {
        // 关闭是独立动作：表单再怎么无效，也必须能停下来。
        XCTAssertEqual(RotationPlanControl.steps(enabled: false, dirty: true), [.apply])
        XCTAssertEqual(RotationPlanControl.steps(enabled: false, dirty: false), [.apply])
        XCTAssertEqual(RotationPlanControl.steps(enabled: true, dirty: true), [.save, .apply])
        XCTAssertEqual(RotationPlanControl.steps(enabled: true, dirty: false), [.apply])
    }

    func testStopRequestKeepsItsAsynchronousMeaning() {
        let requested = RotationFeedbackText.stop(status: "stop_requested", restored: nil, detail: nil)
        XCTAssertTrue(requested.text.contains("当前操作结束后"), "停止只是请求，不能宣称已恢复")
        XCTAssertFalse(requested.isFailure)

        let restored = RotationFeedbackText.stop(status: "stopped", restored: true, detail: nil)
        XCTAssertTrue(restored.text.contains("恢复了原账号"))

        let notRestored = RotationFeedbackText.stop(status: "stopped", restored: false, detail: nil)
        XCTAssertTrue(notRestored.isFailure, "恢复失败必须算失败")
    }

    // MARK: - 缓存 + 最新读取失败

    func testEveryPlanKeepsTheCachedValueButMarksItAsStale() {
        var inputs = AutomationInputs()
        inputs.checkinAuto = enabledCheckinAuto()
        inputs.checkinError = "助手超时"
        inputs.growthAuto = GrowthAutoStatus(json: ["enabled": true, "intervalSeconds": 86_400])
        inputs.growthError = "助手超时"
        inputs.travelAuto = TravelAutoStatus(json: [
            "enabledAccounts": 2, "busy": false,
            "departIntervalSecs": 1800, "claimIntervalSecs": 600,
            "owner": "AICC.app", "runsOnlyWhileAppRunning": true,
        ])
        inputs.travelError = "助手超时"
        inputs.creditsLoaded = true
        inputs.creditsAuto = [
            QoderCreditsAutoItem(json: ["accountId": "a", "autoEnabled": true, "supported": true])!,
        ]
        inputs.creditsError = "助手超时"
        inputs.rotation = rotationStatus(enabled: true, accountIds: ["a"])
        inputs.rotationError = "助手超时"

        for row in AutomationPlanBuilder.rows(inputs) {
            XCTAssertTrue(row.state.isStale, "\(row.kind.rawValue) 有缓存但读取失败必须是过期状态")
            XCTAssertTrue(row.statusText.hasPrefix("上次："), row.statusText)
            XCTAssertTrue(row.summary.contains("当前无法确认"), row.summary)
            XCTAssertEqual(row.tone, .warning)
            XCTAssertNil(row.nextRun, "过期的下次执行时间不能当成当前排期")
        }
    }

    func testNoCacheAndFailureIsAFailureNotStale() {
        var inputs = AutomationInputs()
        inputs.checkinError = "助手未运行"
        inputs.growthError = "助手未运行"
        inputs.travelError = "助手未运行"
        inputs.creditsError = "助手未运行"
        inputs.rotationError = "助手未运行"

        for row in AutomationPlanBuilder.rows(inputs) {
            guard case .failed = row.state else {
                return XCTFail("\(row.kind.rawValue) 没有缓存时必须是 failed")
            }
            XCTAssertEqual(row.tone, .error)
            XCTAssertFalse(row.statusText.hasPrefix("上次："))
        }
    }

    func testLoadingWithACacheDoesNotLookLikeOff() {
        var inputs = AutomationInputs()
        inputs.checkinAuto = enabledCheckinAuto()
        inputs.checkinLoading = true
        inputs.growthAuto = GrowthAutoStatus(json: ["enabled": true])
        inputs.growthLoading = true
        inputs.travelAuto = TravelAutoStatus(json: [
            "enabledAccounts": 1, "busy": false,
            "departIntervalSecs": 1800, "claimIntervalSecs": 600,
            "owner": "AICC.app", "runsOnlyWhileAppRunning": true,
        ])
        inputs.travelLoading = true
        inputs.creditsLoaded = true
        inputs.creditsAuto = [
            QoderCreditsAutoItem(json: ["accountId": "a", "autoEnabled": true, "supported": true])!,
        ]
        inputs.creditsLoading = true
        inputs.rotation = rotationStatus(enabled: true, accountIds: ["a"])
        inputs.rotationLoading = true

        for row in AutomationPlanBuilder.rows(inputs) {
            XCTAssertNotEqual(row.state, .off, "\(row.kind.rawValue) 读取中不能显示成未开启")
            XCTAssertNotEqual(row.state, .loading, "\(row.kind.rawValue) 有缓存时不该丢掉已知状态")
        }
    }

    func testRotationStaleKeepsTheLastKnownPlanButDropsNextRun() {
        var inputs = AutomationInputs()
        inputs.rotation = rotationStatus(enabled: true, accountIds: ["a"])
        inputs.rotationError = "助手超时"

        let row = AutomationPlanBuilder.rotation(inputs, now: Date())
        XCTAssertTrue(row.state.isStale)
        XCTAssertEqual(row.statusText, "上次：已开启")
        XCTAssertTrue(row.summary.contains("已开启 · 1 个账号"))
        XCTAssertTrue(row.summary.contains("当前无法确认"))
        XCTAssertNil(row.nextRun)
    }

    // MARK: - 预演只对已保存计划

    func testDirtyDraftCannotClaimAServerPreview() {
        let state = RotationPreviewState(
            dirty: true,
            planEnabled: true,
            savedPlanKey: "saved",
            dryRunPlanKey: "saved",
            hasDryRun: true
        )
        XCTAssertFalse(state.canPreview, "草稿没保存时不能预演")
        XCTAssertFalse(state.showsDryRun, "旧计划的预演不能冒充新草稿的验证结果")
        XCTAssertEqual(state.summaryTitle, "未保存的配置（本地摘要）")
        XCTAssertTrue(state.caveat?.contains("仅保存") == true)
        XCTAssertTrue(state.caveat?.contains("计划当前是开启的") == true,
                      "开启中的计划保存会立刻生效，必须说明")
    }

    func testPreviewOnlyCountsWhenItMatchesTheSavedPlan() {
        let matching = RotationPreviewState(
            dirty: false,
            planEnabled: false,
            savedPlanKey: "a|b",
            dryRunPlanKey: "a|b",
            hasDryRun: true
        )
        XCTAssertTrue(matching.canPreview)
        XCTAssertTrue(matching.showsDryRun)
        XCTAssertFalse(matching.showsStaleResult)
        XCTAssertTrue(matching.caveat?.contains("不代表真实切换一定成功") == true)

        let stale = RotationPreviewState(
            dirty: false,
            planEnabled: false,
            savedPlanKey: "new",
            dryRunPlanKey: "old",
            hasDryRun: true
        )
        XCTAssertTrue(stale.showsStaleResult)
        XCTAssertFalse(stale.showsDryRun)
        XCTAssertTrue(stale.caveat?.contains("过期") == true)

        let none = RotationPreviewState(
            dirty: false,
            planEnabled: false,
            savedPlanKey: "new",
            dryRunPlanKey: nil,
            hasDryRun: false
        )
        XCTAssertTrue(none.canPreview)
        XCTAssertFalse(none.showsDryRun)
        XCTAssertTrue(none.caveat?.contains("还没有预演") == true)
    }

    func testPlanKeyCoversEveryPreviewFieldAndIgnoresEnabled() {
        let plan = rotationPlan(enabled: false)
        var draft = RotationDraft(plan: plan)
        XCTAssertEqual(draft.planKey, plan.planKey)
        XCTAssertEqual(RotationDraft(plan: rotationPlan(enabled: true)).planKey, draft.planKey,
                       "开关不影响 dry-run 内容")

        draft.accountIds = ["b", "a"]
        XCTAssertNotEqual(draft.planKey, plan.planKey, "顺序变了就是另一份计划")

        draft = RotationDraft(plan: plan)
        draft.startMinutes += 1
        XCTAssertNotEqual(draft.planKey, plan.planKey)

        draft = RotationDraft(plan: plan)
        draft.dwellSeconds = 600
        XCTAssertNotEqual(draft.planKey, plan.planKey)

        draft = RotationDraft(plan: plan)
        draft.weekdays = [6]
        XCTAssertNotEqual(draft.planKey, plan.planKey)

        draft = RotationDraft(plan: plan)
        draft.timezone = "UTC"
        XCTAssertNotEqual(draft.planKey, plan.planKey)
    }

    // MARK: - 离开编辑

    func testLeavePolicyNeverLosesEditsAndNeverFightsASave() {
        XCTAssertEqual(RotationLeavePolicy.decide(intent: .close, dirty: false, saving: false), .proceed)
        XCTAssertEqual(RotationLeavePolicy.decide(intent: .close, dirty: true, saving: false), .confirm)
        XCTAssertEqual(RotationLeavePolicy.decide(intent: .tab(.history), dirty: true, saving: false), .confirm)
        XCTAssertEqual(RotationLeavePolicy.decide(intent: .cancelEditing, dirty: true, saving: false), .confirm)
        XCTAssertEqual(RotationLeavePolicy.decide(intent: .close, dirty: true, saving: true), .ignore,
                       "保存执行中不接受会丢状态的导航")
        XCTAssertEqual(RotationLeavePolicy.decide(intent: .tab(.history), dirty: false, saving: true), .ignore)
    }

    // MARK: - 文案

    func testWeekdayTextCollapsesRuns() {
        XCTAssertEqual(AutomationScheduleText.weekdays([1, 2, 3, 4, 5, 6, 7]), "每天")
        XCTAssertEqual(AutomationScheduleText.weekdays([1, 2, 3, 4, 5]), "周一至周五")
        XCTAssertEqual(AutomationScheduleText.weekdays([1, 3]), "周一、周三")
        XCTAssertEqual(AutomationScheduleText.weekdays([]), "未选星期")
    }

    func testShortTimeIsOnlyProducedWhenThereIsADate() {
        XCTAssertNil(AutomationTime.short(nil))
        let text = AutomationTime.short(Date(timeIntervalSince1970: 0))
        XCTAssertNotNil(text)
        XCTAssertEqual(text?.count, 11, "MM-dd HH:mm")
    }

    // MARK: - 样本

    private func rotationPlan(enabled: Bool) -> WorkbuddyRotationPlan {
        WorkbuddyRotationPlan(json: [
            "enabled": enabled,
            "timezone": "Asia/Shanghai",
            "weekdays": [1, 2, 3, 4, 5],
            "startMinutes": 22 * 60 + 30,
            "endMinutes": 7 * 60,
            "accountIds": ["a", "b"],
            "dwellSeconds": 3600,
            "startText": "22:30",
            "endText": "07:00",
        ])
    }

    private func rotationStatus(enabled: Bool, accountIds: [String]) -> WorkbuddyRotationStatus {
        WorkbuddyRotationStatus(json: [
            "plan": [
                "enabled": enabled,
                "timezone": "Asia/Shanghai",
                "weekdays": accountIds.isEmpty ? [] : [1, 2, 3, 4, 5],
                "startMinutes": 22 * 60 + 30,
                "endMinutes": 7 * 60,
                "accountIds": accountIds,
                "dwellSeconds": 3600,
            ],
            "schedule": ["inWindow": false, "nextRunAt": "2026-11-21T14:30:00Z"],
            "live": ["uid": "wb_1", "loggedIn": true],
            "history": [],
            "limits": ["supportedTimezones": ["Asia/Shanghai"]],
        ])
    }

    private func enabledCheckinAuto() -> CheckinAutoStatus {
        var auto = CheckinAutoStatus()
        auto.enabled = true
        auto.enabledAccounts = 2
        auto.totalAccounts = 3
        return auto
    }

    private func rotationDwellingStatus(enabled: Bool) -> WorkbuddyRotationStatus {
        WorkbuddyRotationStatus(json: [
            "plan": [
                "enabled": enabled,
                "timezone": "Asia/Shanghai",
                "weekdays": [1, 2],
                "startMinutes": 22 * 60 + 30,
                "endMinutes": 7 * 60,
                "accountIds": ["a"],
                "dwellSeconds": 3600,
            ],
            "schedule": ["inWindow": true],
            "live": ["uid": "wb_1", "loggedIn": true],
            "current": [
                "id": "r0",
                "planDate": "2026-11-21",
                "state": "dwelling",
                "restoreResult": "等待恢复",
                "steps": [],
            ],
            "history": [],
        ])
    }

    private func rotationRecoveryStatus(enabled: Bool = true) -> WorkbuddyRotationStatus {
        WorkbuddyRotationStatus(json: [
            "plan": [
                "enabled": enabled,
                "timezone": "Asia/Shanghai",
                "weekdays": [1, 2, 3],
                "startMinutes": 22 * 60 + 30,
                "endMinutes": 7 * 60,
                "accountIds": ["a", "b"],
                "dwellSeconds": 3600,
            ],
            "schedule": ["inWindow": false],
            "live": ["uid": "wb_1", "loggedIn": true],
            // 后端把 recovery_required 排除在活动轮次之外：current 为 nil，只有 lastRun。
            "lastRun": [
                "id": "r1",
                "planDate": "2026-11-20",
                "state": "recovery_required",
                "restoreResult": "restore_failed",
                "detail": "已写回快照，但客户端当前登录不是原账号",
                "steps": [],
            ],
            "history": [],
        ])
    }
}
