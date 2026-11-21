import Combine
import Foundation

/// 轮换管理界面需要向 store 发的动作。
///
/// 抽成协议不是为了"架构好看"，而是为了让测试跑的是**视图真正调用的那条路径**：
/// 生产实现是 `ConsoleStore`，测试实现是内存替身。视图不直接碰 `ConsoleStore`
/// 的写方法，只经过这里。
@MainActor
protocol RotationControlling: AnyObject {
    var wbRotationStatus: WorkbuddyRotationStatus? { get }
    var wbRotationError: String? { get }
    var wbRotationNotice: String? { get }
    var wbRotationLoading: Bool { get }
    var wbRotationDryRun: WorkbuddyRotationDryRun? { get }
    var wbRotationDryRunPlanKey: String? { get }

    func refreshWorkbuddyRotation(silent: Bool) async
    func saveWorkbuddyRotation(_ params: [String: Any], note: String?) async -> Bool
    func setWorkbuddyRotationEnabled(_ enabled: Bool) async -> Bool
    func previewWorkbuddyRotation() async
}

/// 首次开启前的风险确认要落到哪个动作上。
enum RotationPendingEnable: Equatable {
    case planToggle
    case saveAndEnable
}

/// 自动轮换管理界面的状态机。
///
/// 视图只负责画，所有"该不该问、该不该发请求"的判断都在这里，
/// 这样它们能被确定性测试覆盖，而不是埋在 `Button` 的闭包里。
@MainActor
final class RotationManagerModel: ObservableObject {
    let store: RotationControlling
    private let focus: RotationManagerFocus
    private let initialStep: RotationSetupStep
    /// 离屏渲染用的初始草稿。生产调用不传。
    private let seedDraft: RotationDraft?

    @Published private(set) var tab: RotationManagerTab
    @Published private(set) var editing = false
    @Published var step: RotationSetupStep = .accounts
    @Published private(set) var draft: RotationDraft?
    @Published private(set) var saving = false
    @Published private(set) var saveFailure: String?
    @Published private(set) var localHint: String?
    /// 等待确认的离开意图。非 nil 时界面弹"放弃未保存的修改"。
    @Published private(set) var pendingLeave: RotationLeaveIntent?
    /// 关闭窗口的请求。视图监听它并调用 `dismiss`。
    @Published private(set) var closeRequested = false
    @Published private(set) var togglingPlan = false
    /// 开关失败的最后一句话。展示仍以 store 的 notice 为准，这里供测试与后续入口使用。
    @Published private(set) var toggleFailure: String?
    /// 首次开启的确认。非 nil 时界面弹风险说明。
    @Published private(set) var pendingEnable: RotationPendingEnable?

    init(
        store: RotationControlling,
        focus: RotationManagerFocus = .plan,
        initialStep: RotationSetupStep = .accounts,
        seedDraft: RotationDraft? = nil
    ) {
        self.store = store
        self.focus = focus
        self.initialStep = initialStep
        self.seedDraft = seedDraft
        tab = focus == .history ? .history : .plan
    }

    // MARK: - 派生

    var status: WorkbuddyRotationStatus? { store.wbRotationStatus }
    var plan: WorkbuddyRotationPlan? { status?.plan }
    var planEnabled: Bool? { plan?.enabled }

    var overview: RotationOverview {
        RotationOverview.make(
            status: status,
            loading: store.wbRotationLoading,
            error: store.wbRotationError
        )
    }

    var dirty: Bool {
        guard let draft, let plan else { return false }
        return draft.isDirty(against: plan)
    }

    var preview: RotationPreviewState {
        RotationPreviewState(
            dirty: dirty,
            planEnabled: plan?.enabled == true,
            savedPlanKey: plan?.planKey,
            dryRunPlanKey: store.wbRotationDryRunPlanKey,
            hasDryRun: store.wbRotationDryRun != nil
        )
    }

    /// 首次开启才需要风险确认。已经开启的计划刷新多少次都不再弹。
    var firstEnableNeedsConfirmation: Bool { plan?.enabled != true }

    // MARK: - 生命周期

    func handleFocus() {
        if focus == .setup, status != nil, !editing {
            beginEditing()
        }
    }

    func beginEditing() {
        guard let plan else { return }
        draft = seedDraft ?? RotationDraft(plan: plan)
        step = initialStep
        saveFailure = nil
        localHint = nil
        editing = true
        tab = .plan
    }

    func replaceDraft(_ new: RotationDraft) {
        draft = new
    }

    // MARK: - 离开

    func requestLeave(_ intent: RotationLeaveIntent) {
        switch RotationLeavePolicy.decide(intent: intent, dirty: dirty, saving: saving) {
        case .ignore:
            return
        case .proceed:
            apply(intent)
        case .confirm:
            pendingLeave = intent
        }
    }

    func requestTab(_ target: RotationManagerTab) {
        guard target != tab else { return }
        requestLeave(.tab(target))
    }

    func requestClose() {
        requestLeave(.close)
    }

    func confirmLeave() {
        guard let intent = pendingLeave else { return }
        pendingLeave = nil
        apply(intent)
    }

    func cancelLeave() {
        pendingLeave = nil
    }

    private func apply(_ intent: RotationLeaveIntent) {
        switch intent {
        case .close:
            closeRequested = true
        case .cancelEditing:
            resetEditing()
        case .tab(let target):
            resetEditing()
            tab = target
        }
    }

    private func resetEditing() {
        editing = false
        draft = nil
        step = .accounts
        saveFailure = nil
        localHint = nil
    }

    // MARK: - 开关

    /// 从界面请求开启/关闭。关闭不需要确认；开启在计划本来关闭时先弹风险说明。
    func requestPlanEnabled(_ enabled: Bool) {
        guard plan != nil else { return }
        if enabled, firstEnableNeedsConfirmation {
            pendingEnable = .planToggle
            return
        }
        Task { await setPlanEnabled(enabled) }
    }

    @discardableResult
    func setPlanEnabled(_ enabled: Bool) async -> RotationToggleOutcome {
        guard !togglingPlan else { return .applyFailed }
        togglingPlan = true
        toggleFailure = nil
        defer { togglingPlan = false }

        let outcome = await RotationPlanControl.setPlanEnabled(
            enabled,
            dirty: dirty,
            save: { [store, draft] in
                guard let draft else { return false }
                return await store.saveWorkbuddyRotation(draft.params, note: nil)
            },
            apply: { [store] value in
                await store.setWorkbuddyRotationEnabled(value)
            },
            reload: { [store] in
                await store.refreshWorkbuddyRotation(silent: true)
            }
        )

        switch outcome {
        case .saveFailed:
            toggleFailure = store.wbRotationNotice ?? "计划没有保存成功。"
        case .applyFailed:
            toggleFailure = store.wbRotationNotice ?? "开关没有保存成功。"
        case .applied:
            toggleFailure = nil
        }
        return outcome
    }

    // MARK: - 保存

    func requestSaveOnly() {
        Task { await performSave(enable: false) }
    }

    /// 「保存并开启」。计划本来关闭时先弹风险确认；取消不会保存也不会开启。
    func requestSaveAndEnable() {
        guard let plan else { return }
        if !plan.enabled {
            pendingEnable = .saveAndEnable
            return
        }
        Task { await performSave(enable: true) }
    }

    /// 确认首次开启。异步是为了让测试能等到动作真正完成，而不是断言一个刚起的 Task。
    func confirmFirstEnable() async {
        guard let action = pendingEnable else { return }
        pendingEnable = nil
        switch action {
        case .planToggle:
            await setPlanEnabled(true)
        case .saveAndEnable:
            await performSave(enable: true)
        }
    }

    func cancelFirstEnable() {
        pendingEnable = nil
    }

    @discardableResult
    func performSave(enable: Bool) async -> RotationSaveResult? {
        guard let draft, let plan else { return nil }
        saving = true
        saveFailure = nil
        localHint = nil

        let result = await RotationSaveFlow.run(
            dirty: draft.isDirty(against: plan),
            enable: enable,
            save: { [store] in
                await store.saveWorkbuddyRotation(draft.params, note: nil)
            },
            setEnabled: { [store] value in
                await store.setWorkbuddyRotationEnabled(value)
            },
            reload: { [store] in
                await store.refreshWorkbuddyRotation(silent: true)
            }
        )

        saving = false
        switch result {
        case .savedOnly:
            localHint = "计划已保存（还没有开启）。预演现在针对的就是这份已保存计划。"
        case .enabled:
            resetEditing()
        case .saveFailed:
            // 保存失败：保留表单内容，绝不用旧计划去开启。
            saveFailure = store.wbRotationNotice ?? "计划没有保存成功。"
        case .enableFailed:
            // 已经回读过服务端状态；这里只说明没有开启成功。
            saveFailure = store.wbRotationNotice.map { "开启失败：\($0)" }
                ?? "开启失败，计划仍是原来的状态。"
        }
        return result
    }

    // MARK: - 预演

    #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
    /// 离屏渲染：直接呈现保存/启用失败的底部提示。生产构建里没有这个方法。
    func applyVisualFailure(_ message: String) {
        saveFailure = message
    }
    #endif

    func previewSavedPlan() async {
        guard preview.canPreview else { return }
        await store.previewWorkbuddyRotation()
    }
}
