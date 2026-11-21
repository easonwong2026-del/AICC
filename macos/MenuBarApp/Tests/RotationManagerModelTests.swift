import XCTest
@testable import AICCCore

/// 轮换管理界面的**生产控制路径**。
///
/// 视图不直接调 `ConsoleStore` 的写方法，只调 `RotationManagerModel`；
/// 模型只通过 `RotationControlling` 发动作。这里用替身跑同一条路径，
/// 断言"点了之后到底发了什么、什么时候没发"。
@MainActor
final class RotationManagerModelTests: XCTestCase {

    // MARK: - P0-1 关闭

    func testDisableNeverSavesTheFormEvenWhenTheDraftIsInvalid() async {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        // 故意把草稿改成无效：没有账号、没有星期。
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: true)).with {
            $0.accountIds = []
            $0.weekdays = []
        })

        let outcome = await model.setPlanEnabled(false)

        XCTAssertEqual(outcome, .applied(enabled: false))
        XCTAssertEqual(store.calls, ["enabled:false"], "关闭不能先保存一个无效表单")
        XCTAssertFalse(store.calls.contains("save"))
    }

    func testDisableIsAllowedWhileARoundIsStillWindingDown() async {
        let store = FakeRotationStore(status: Self.status(enabled: true, dwelling: true))
        let model = RotationManagerModel(store: store)
        XCTAssertEqual(model.status?.current?.isTerminal, false)

        let outcome = await model.setPlanEnabled(false)

        XCTAssertEqual(outcome, .applied(enabled: false))
        XCTAssertEqual(store.calls, ["enabled:false"])
        XCTAssertEqual(model.planEnabled, false)
    }

    func testDisableFailureDoesNotLookLikeSuccess() async {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        store.enableSucceeds = false
        let model = RotationManagerModel(store: store)

        let outcome = await model.setPlanEnabled(false)

        XCTAssertEqual(outcome, .applyFailed)
        XCTAssertEqual(store.calls, ["enabled:false", "refresh:true"], "失败必须回读真实状态")
        XCTAssertEqual(model.planEnabled, true, "服务端没关掉，界面不能显示已关闭")
        XCTAssertNotNil(model.toggleFailure)
    }

    // MARK: - P0-1 开启

    func testEnableWithDirtyDraftSavesFirstThenEnables() async {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: false)).with {
            $0.startMinutes = 21 * 60
        })

        let outcome = await model.setPlanEnabled(true)

        XCTAssertEqual(outcome, .applied(enabled: true))
        XCTAssertEqual(store.calls, ["save", "enabled:true"])
    }

    func testEnableWithAFailedSaveNeverTurnsThePlanOn() async {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        store.saveSucceeds = false
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: false)).with {
            $0.startMinutes = 21 * 60
        })

        let outcome = await model.setPlanEnabled(true)

        XCTAssertEqual(outcome, .saveFailed)
        XCTAssertEqual(store.calls, ["save"], "保存失败后不能发开启请求")
        XCTAssertEqual(model.planEnabled, false)
        XCTAssertNotNil(model.toggleFailure)
    }

    func testWorkerUnavailableNeverPretendsTheSwitchChanged() async {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        store.enableSucceeds = false
        store.enableFailureNotice = "助手未运行，无法修改计划"
        let model = RotationManagerModel(store: store)

        let outcome = await model.setPlanEnabled(true)

        XCTAssertEqual(outcome, .applyFailed)
        XCTAssertEqual(model.planEnabled, false)
        XCTAssertFalse(model.overview.state.isOnOrRunning)
        XCTAssertEqual(model.toggleFailure, "助手未运行，无法修改计划")
    }

    // MARK: - P1-1 首次开启确认

    func testFirstEnableAlwaysAsksBeforeTouchingAnything() {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        let model = RotationManagerModel(store: store)

        model.requestPlanEnabled(true)

        XCTAssertEqual(model.pendingEnable, .planToggle)
        XCTAssertTrue(store.calls.isEmpty, "确认之前不能发任何请求")

        model.cancelFirstEnable()
        XCTAssertNil(model.pendingEnable)
        XCTAssertTrue(store.calls.isEmpty, "取消后也不能保存或启用")
    }

    func testFirstEnableRunsOnlyAfterConfirmation() async {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        let model = RotationManagerModel(store: store)

        model.requestPlanEnabled(true)
        XCTAssertTrue(store.calls.isEmpty)

        await model.confirmFirstEnable()

        XCTAssertNil(model.pendingEnable)
        XCTAssertEqual(store.calls, ["enabled:true"])
        XCTAssertEqual(model.planEnabled, true)
    }

    func testFirstEnableFromSaveAndEnableAlsoWaitsForConfirmation() async {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: false)).with { $0.startMinutes = 21 * 60 })

        model.requestSaveAndEnable()
        XCTAssertEqual(model.pendingEnable, .saveAndEnable)
        XCTAssertTrue(store.calls.isEmpty)
        XCTAssertTrue(model.editing, "取消/未确认时表单还在")

        model.cancelFirstEnable()
        XCTAssertTrue(store.calls.isEmpty)
        XCTAssertTrue(model.editing)

        model.requestSaveAndEnable()
        await model.confirmFirstEnable()
        XCTAssertEqual(store.calls, ["save", "enabled:true", "refresh:true"],
                       "确认后才按顺序保存、开启并回读")
    }

    func testAnAlreadyEnabledPlanDoesNotAskAgain() async {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: true)).with { $0.startMinutes = 21 * 60 })

        XCTAssertFalse(model.firstEnableNeedsConfirmation)
        XCTAssertNil(model.pendingEnable, "已开启的计划保存不该再弹首次风险确认")

        await model.performSave(enable: true)
        XCTAssertEqual(store.calls, ["save", "enabled:true", "refresh:true"])
    }

    /// P2：仅保存的提示按服务端实际开关说，已开启不能说"还没有开启"。
    func testSaveOnlyCopyFollowsTheServerSwitch() async {
        let onStore = FakeRotationStore(status: Self.status(enabled: true))
        let onModel = RotationManagerModel(store: onStore)
        onModel.beginEditing()
        onModel.replaceDraft(RotationDraft(plan: Self.plan(enabled: true)).with { $0.startMinutes = 21 * 60 })
        await onModel.performSave(enable: false)
        XCTAssertEqual(onModel.localHint, "计划已保存。新的配置已经生效。")

        let offStore = FakeRotationStore(status: Self.status(enabled: false))
        let offModel = RotationManagerModel(store: offStore)
        offModel.beginEditing()
        offModel.replaceDraft(RotationDraft(plan: Self.plan(enabled: false)).with { $0.startMinutes = 21 * 60 })
        await offModel.performSave(enable: false)
        XCTAssertEqual(offModel.localHint, "计划已保存（还没有开启）。预演现在针对的就是这份已保存计划。")
    }

    // MARK: - P1-3 未保存修改的离开

    func testCloseWithUnsavedChangesAsksThenReallyCloses() {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: true)).with { $0.startMinutes = 21 * 60 })

        model.requestClose()
        XCTAssertEqual(model.pendingLeave, .close)
        XCTAssertFalse(model.closeRequested)
        XCTAssertTrue(model.editing, "确认之前保留编辑内容")

        model.confirmLeave()
        XCTAssertTrue(model.closeRequested, "确认放弃后必须真的关闭窗口")
    }

    func testCancelCloseKeepsTheEdits() {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        let edited = RotationDraft(plan: Self.plan(enabled: true)).with { $0.startMinutes = 21 * 60 }
        model.replaceDraft(edited)

        model.requestClose()
        model.cancelLeave()

        XCTAssertNil(model.pendingLeave)
        XCTAssertFalse(model.closeRequested)
        XCTAssertTrue(model.editing)
        XCTAssertEqual(model.draft, edited, "取消关闭不能丢修改")
    }

    func testTabSwitchWaitsForConfirmationAndCancellingStaysOnThePlan() {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: true)).with { $0.weekdays = [1] })

        model.requestTab(.history)
        XCTAssertEqual(model.pendingLeave, .tab(.history))
        XCTAssertEqual(model.tab, .plan, "确认之前不能先切走")
        XCTAssertTrue(model.editing)

        model.cancelLeave()
        XCTAssertEqual(model.tab, .plan, "取消后必须回到原来的编辑页")
        XCTAssertTrue(model.editing)

        model.requestTab(.history)
        model.confirmLeave()
        XCTAssertEqual(model.tab, .history)
        XCTAssertFalse(model.editing)
    }

    func testTabSwitchWithoutChangesGoesStraightThere() {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        let model = RotationManagerModel(store: store)
        model.beginEditing()

        model.requestTab(.history)

        XCTAssertNil(model.pendingLeave)
        XCTAssertEqual(model.tab, .history)
        XCTAssertFalse(model.editing)
    }

    func testCloseWithoutChangesClosesImmediately() {
        let store = FakeRotationStore(status: Self.status(enabled: true))
        let model = RotationManagerModel(store: store)

        model.requestClose()

        XCTAssertNil(model.pendingLeave)
        XCTAssertTrue(model.closeRequested)
    }

    func testSavingBlocksConflictingNavigation() {
        // 保存中途不允许关闭/切页：这条是模型使用的同一个判定函数。
        XCTAssertEqual(
            RotationLeavePolicy.decide(intent: .close, dirty: true, saving: true),
            .ignore
        )
        XCTAssertEqual(
            RotationLeavePolicy.decide(intent: .tab(.history), dirty: false, saving: true),
            .ignore
        )
    }

    // MARK: - P0-2 预演与草稿

    func testPreviewIsBlockedWhileTheDraftIsUnsaved() {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        store.wbRotationDryRun = Self.dryRun
        store.wbRotationDryRunPlanKey = Self.plan(enabled: false).planKey
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        model.replaceDraft(RotationDraft(plan: Self.plan(enabled: false)).with { $0.startMinutes = 21 * 60 })

        XCTAssertFalse(model.preview.canPreview)
        XCTAssertFalse(model.preview.showsDryRun, "旧结果不能当成新草稿的验证")
    }

    func testPreviewMatchesTheSavedPlanAfterSaving() {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        store.wbRotationDryRun = Self.dryRun
        store.wbRotationDryRunPlanKey = Self.plan(enabled: false).planKey
        let model = RotationManagerModel(store: store)
        model.beginEditing()

        XCTAssertTrue(model.preview.canPreview)
        XCTAssertTrue(model.preview.showsDryRun)
    }

    func testEditingAfterAPreviewHidesTheOldResultAndRevertingBringsItBack() {
        let store = FakeRotationStore(status: Self.status(enabled: false))
        store.wbRotationDryRun = Self.dryRun
        store.wbRotationDryRunPlanKey = Self.plan(enabled: false).planKey
        let model = RotationManagerModel(store: store)
        model.beginEditing()
        let original = RotationDraft(plan: Self.plan(enabled: false))

        model.replaceDraft(original.with { $0.timezone = "UTC" })
        XCTAssertFalse(model.preview.showsDryRun)

        model.replaceDraft(original)
        XCTAssertTrue(model.preview.showsDryRun, "改回已保存的值，结果仍对这份计划有效")
    }

    // MARK: - 焦点

    func testFocusDecidesWhichTabOpens() {
        let history = RotationManagerModel(store: FakeRotationStore(status: Self.status(enabled: true)),
                                           focus: .history)
        XCTAssertEqual(history.tab, .history)

        let plan = RotationManagerModel(store: FakeRotationStore(status: Self.status(enabled: true)),
                                        focus: .plan)
        XCTAssertEqual(plan.tab, .plan)
        XCTAssertFalse(plan.editing)
    }

    func testSetupFocusStartsEditingOnlyAfterStatusArrives() {
        let store = FakeRotationStore(status: nil)
        let model = RotationManagerModel(store: store, focus: .setup)
        model.handleFocus()
        XCTAssertFalse(model.editing, "状态还没读到时不进编辑")

        store.wbRotationStatus = Self.status(enabled: false)
        model.handleFocus()
        XCTAssertTrue(model.editing)
        XCTAssertNotNil(model.draft)
    }

    // MARK: - 样本

    private static func plan(enabled: Bool) -> WorkbuddyRotationPlan {
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

    fileprivate static func status(enabled: Bool, dwelling: Bool = false) -> WorkbuddyRotationStatus {
        var payload: [String: Any] = [
            "plan": [
                "enabled": enabled,
                "timezone": "Asia/Shanghai",
                "weekdays": [1, 2, 3, 4, 5],
                "startMinutes": 22 * 60 + 30,
                "endMinutes": 7 * 60,
                "accountIds": ["a", "b"],
                "dwellSeconds": 3600,
            ],
            "schedule": ["inWindow": dwelling],
            "live": ["uid": "wb_1", "loggedIn": true],
            "history": [],
        ]
        if dwelling {
            payload["current"] = [
                "id": "run-1",
                "planDate": "2026-11-21",
                "state": "dwelling",
                "restoreResult": "等待恢复",
                "steps": [],
            ]
        }
        return WorkbuddyRotationStatus(json: payload)
    }

    private static var dryRun: WorkbuddyRotationDryRun {
        WorkbuddyRotationDryRun(json: [
            "steps": [[
                "accountId": "a",
                "outcome": "switch",
                "plannedAt": "2026-11-21T14:30:00Z",
            ]],
            "warnings": [],
        ])
    }
}

// MARK: - 替身

@MainActor
private final class FakeRotationStore: RotationControlling {
    var wbRotationStatus: WorkbuddyRotationStatus?
    var wbRotationError: String?
    var wbRotationNotice: String?
    var wbRotationLoading = false
    var wbRotationDryRun: WorkbuddyRotationDryRun?
    var wbRotationDryRunPlanKey: String?

    var saveSucceeds = true
    var enableSucceeds = true
    var enableFailureNotice: String?
    private(set) var calls: [String] = []

    init(status: WorkbuddyRotationStatus?) {
        wbRotationStatus = status
    }

    func refreshWorkbuddyRotation(silent: Bool) async {
        calls.append("refresh:\(silent)")
    }

    func saveWorkbuddyRotation(_ params: [String: Any], note: String?) async -> Bool {
        calls.append("save")
        if !saveSucceeds {
            wbRotationNotice = "保存失败：后端拒绝。设置没有改动。"
        }
        return saveSucceeds
    }

    func setWorkbuddyRotationEnabled(_ enabled: Bool) async -> Bool {
        calls.append("enabled:\(enabled)")
        guard enableSucceeds else {
            wbRotationNotice = enableFailureNotice ?? "助手未运行"
            return false
        }
        wbRotationStatus = RotationManagerModelTests.status(enabled: enabled)
        return true
    }

    func previewWorkbuddyRotation() async {
        calls.append("preview")
    }

}

// MARK: - 小工具

private extension RotationDraft {
    /// 让测试改一个字段时不用写六行 var。
    func with(_ mutate: (inout RotationDraft) -> Void) -> RotationDraft {
        var copy = self
        mutate(&copy)
        return copy
    }
}
