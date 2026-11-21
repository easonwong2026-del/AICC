import SwiftUI

/// 自动轮换管理：日常只看计划与结果，改配置才进分步编辑。
///
/// 与旧弹层的区别不是尺寸，而是**职责**：
/// - 打开默认是概览（开没开、几个账号、什么时候跑、上次怎样、要不要处理）；
/// - 配置走三步（账号 → 时间 → 预演/保存），一次设置好；
/// - 执行操作（立即执行 / 停止 / 恢复）单独收在「执行」区，不和保存并排。
struct RotationManagerView: View {
    @ObservedObject var store: ConsoleStore
    let focus: RotationManagerFocus

    @Environment(\.dismiss) private var dismiss

    enum Tab: String, CaseIterable, Identifiable {
        case plan
        case history

        var id: String { rawValue }
        var title: String { self == .plan ? "计划" : "执行记录" }
    }

    @State private var tab: Tab
    @State private var editing = false
    @State private var step: RotationSetupStep = .accounts
    @State private var draft: RotationDraft?
    @State private var showExecution = false
    @State private var confirmRunNow = false
    @State private var confirmRestore = false
    @State private var confirmDiscard = false
    @State private var saving = false
    @State private var saveFailure: String?
    @State private var localHint: String?
    /// 打开编辑时落在哪一步。生产固定第一步；离屏渲染可以直接看第 2、3 步。
    private let initialStep: RotationSetupStep
    /// 离屏渲染：直接呈现保存/启用失败。生产构建里没有这个参数。
    #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
    var visualSaveFailure: String?
    #endif

    init(
        store: ConsoleStore,
        focus: RotationManagerFocus = .plan,
        initialStep: RotationSetupStep = .accounts
    ) {
        self.store = store
        self.focus = focus
        self.initialStep = initialStep
        _tab = State(initialValue: focus == .history ? .history : .plan)
        _showExecution = State(initialValue: focus == .recovery)
        _editing = State(initialValue: focus == .setup)
    }

    private var status: WorkbuddyRotationStatus? { store.wbRotationStatus }
    private var overview: RotationOverview {
        RotationOverview.make(
            status: status,
            loading: store.wbRotationLoading,
            error: store.wbRotationError
        )
    }

    private var dirty: Bool {
        guard let draft, let plan = status?.plan else { return false }
        return draft.isDirty(against: plan)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            tabBar
            Divider().overlay(ConsoleTheme.border)

            ScrollView {
                VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                    if tab == .plan {
                        if editing {
                            setupContent
                        } else {
                            overviewContent
                        }
                    } else {
                        RotationHistoryView(store: store)
                    }
                }
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 24)
                .frame(maxWidth: 700, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .center)
            }
            .frame(maxHeight: .infinity)

            if editing && tab == .plan {
                Divider().overlay(ConsoleTheme.border)
                setupFooter
            }
        }
        .frame(width: 760, height: 620)
        .background(ConsoleTheme.canvas)
        .onAppear {
            handleFocus()
            #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
            if let visualSaveFailure, editing { saveFailure = visualSaveFailure }
            #endif
        }
        .onChange(of: store.wbRotationStatus) { _, _ in
            // 服务端刷新绝不覆盖正在编辑的表单。
            if !editing { draft = nil }
        }
        .alert("立即执行一轮？", isPresented: $confirmRunNow) {
            Button("取消", role: .cancel) {}
            Button("开始") { Task { await store.runWorkbuddyRotationNow() } }
        } message: {
            Text("AICC 会立刻关闭并重启 WorkBuddy，按顺序切到你选的账号，并在窗口结束时切回最初登录的那个。"
                + "这会占用今天的自动轮换名额；建议只在你在场时用它做一次短时验证。")
        }
        .alert("恢复最初账号？", isPresented: $confirmRestore) {
            Button("取消", role: .cancel) {}
            Button("恢复") { Task { await store.restoreWorkbuddyRotationOrigin(force: true) } }
        } message: {
            Text("会用这一轮开始时保存的认证文件覆盖客户端当前登录态。如果你刚刚手动换过账号，请不要继续。")
        }
        .confirmationDialog("放弃未保存的修改？", isPresented: $confirmDiscard) {
            Button("放弃修改", role: .destructive) { leaveEditing() }
            Button("继续编辑", role: .cancel) {}
        } message: {
            Text("账号顺序与时间还没有保存，离开后这些修改会丢失。")
        }
    }

    // MARK: - 头部与标签

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("自动轮换")
                .font(ConsoleType.detailTitle)
                .foregroundStyle(ConsoleTheme.text)
            Spacer(minLength: 0)
            Button("关闭") { requestClose() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 14)
    }

    private var tabBar: some View {
        HStack(spacing: 8) {
            Picker("", selection: $tab) {
                ForEach(Tab.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 200)
            .onChange(of: tab) { _, newValue in
                if newValue == .history { requestLeaveEditing() }
            }
            Spacer(minLength: 0)
            if editing {
                Text("第 \(step.rawValue + 1)/3 步 · \(step.title)")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 10)
    }

    // MARK: - 概览

    private var overviewContent: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            if let recovery = overview.recoveryText {
                recoveryBanner(recovery)
            }

            if let error = store.wbRotationError, status == nil {
                ConsoleInlineNotice(kind: .error, message: "读不到轮换状态：\(error)", actionTitle: "重试") {
                    Task { await store.refreshWorkbuddyRotation() }
                }
            }

            if let notice = store.wbRotationNotice {
                ConsoleInlineNotice(
                    kind: store.wbRotationFailed ? .error : .info,
                    message: notice
                )
            }

            overviewGroup
            actionRow
            if overview.configured || overview.needsRecovery {
                executionSection
            }
        }
    }

    private var overviewGroup: some View {
        ConsoleListSurface {
            VStack(spacing: 0) {
                overviewRow("状态", value: overview.state.statusText, tone: overview.state.tone)
                rowDivider
                if overview.configured {
                    overviewRow("参与账号", value: "\(overview.accountCount) 个")
                    rowDivider
                    overviewRow("计划时间", value: overview.scheduleText)
                    if let next = overview.nextRunText {
                        rowDivider
                        overviewRow("下次运行", value: next)
                    }
                    if let current = overview.currentText {
                        rowDivider
                        overviewRow("这一轮", value: current)
                    } else if let last = overview.lastResultText, !overview.needsRecovery {
                        // 恢复异常时原因已经在顶部说清楚，这里不再重复一遍。
                        rowDivider
                        overviewRow("最近结果", value: last)
                    }
                } else {
                    // 读不到状态时说"未知"，不能声称"还没有设置"。
                    overviewRow("计划时间", value: overview.scheduleText)
                }
                rowDivider
                overviewRow("客户端登录", value: overview.liveText)
            }
        }
    }

    private var rowDivider: some View {
        Divider().overlay(ConsoleTheme.border)
    }

    private func overviewRow(_ label: String, value: String, tone: SettingsTone = .neutral) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(label)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 76, alignment: .leading)
            Text(value)
                .font(ConsoleType.body)
                .foregroundStyle(toneColor(tone))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 9)
    }

    private var actionRow: some View {
        HStack(spacing: 8) {
            Button(overview.configured ? "设置计划…" : "设置轮换计划…") { beginEditing() }
                .buttonStyle(ConsolePrimaryButtonStyle(enabled: status != nil))
                .disabled(status == nil)
            Spacer(minLength: 0)
        }
    }

    private var executionSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button {
                showExecution.toggle()
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: showExecution ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                    Text("执行操作")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
            }
            .buttonStyle(.plain)

            if showExecution {
                ConsoleListSurface {
                    VStack(spacing: 0) {
                        executionRow(
                            title: "立即执行一轮",
                            detail: "会关闭并重启客户端；占用今天的自动轮换名额。",
                            action: "立即执行…",
                            disabled: store.wbRotationBusy || !overview.configured
                        ) { confirmRunNow = true }

                        rowDivider

                        executionRow(
                            title: "暂停并恢复原账号",
                            detail: "请求可能先落库，等当前切换安全收尾后再恢复。",
                            action: "暂停",
                            disabled: store.wbRotationBusy || status?.current?.isTerminal != false
                        ) {
                            Task { await store.stopWorkbuddyRotation() }
                        }

                        rowDivider

                        executionRow(
                            title: "恢复最初账号",
                            detail: "用这一轮开始时的快照覆盖客户端当前登录。",
                            action: "恢复…",
                            disabled: store.wbRotationBusy
                        ) { confirmRestore = true }
                    }
                }
                .padding(.top, 4)
            }
        }
    }

    private func executionRow(
        title: String,
        detail: String,
        action: String,
        disabled: Bool,
        run: @escaping () -> Void
    ) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text(detail)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Button(action, action: run)
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(disabled)
        }
        .padding(.vertical, 8)
    }

    private func recoveryBanner(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "exclamationmark.triangle")
                    .font(ConsoleType.caption)
                Text("上一轮没有完成恢复")
                    .font(ConsoleType.rowTitle)
                Spacer(minLength: 0)
            }
            .foregroundStyle(ConsoleTheme.warning)
            Text(text)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Text("请先确认客户端现在登录的是哪个账号，再决定是否恢复。")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Spacer(minLength: 8)
                Button("恢复最初账号…") { confirmRestore = true }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ConsoleTheme.warningSoft, in: RoundedRectangle(cornerRadius: ConsoleTheme.controlRadius))
    }

    // MARK: - 编辑内容

    @ViewBuilder private var setupContent: some View {
        if let draft, let plan = status?.plan {
            RotationSetupView(
                store: store,
                draft: Binding(
                    get: { self.draft ?? RotationDraft(plan: plan) },
                    set: { self.draft = $0 }
                ),
                step: step,
                plan: plan
            )
        } else if let error = store.wbRotationError, status == nil {
            ConsoleInlineNotice(kind: .error, message: "读不到轮换计划：\(error)", actionTitle: "重试") {
                Task { await store.refreshWorkbuddyRotation() }
            }
        } else {
            ConsoleLoadingRow(text: "正在读取轮换计划…")
        }
    }

    private var setupFooter: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let saveFailure {
                ConsoleInlineNotice(kind: .error, message: saveFailure)
            } else if let localHint {
                ConsoleInlineNotice(kind: .info, message: localHint)
            } else if dirty {
                Text("有未保存的修改")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.warning)
            }

            HStack(spacing: 8) {
                Button("取消") { requestLeaveEditing() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(saving)

                Spacer(minLength: 8)

                if step.previous != nil {
                    Button("上一步") { step = step.previous ?? .accounts }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(saving)
                }

                if let next = step.next {
                    Button("下一步") { step = next }
                        .buttonStyle(ConsolePrimaryButtonStyle(enabled: canAdvance))
                        .disabled(!canAdvance || saving)
                } else {
                    Button("仅保存") { Task { await performSave(enable: false) } }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(saving || !canSave)
                    Button("保存并开启") { Task { await performSave(enable: true) } }
                        .buttonStyle(ConsolePrimaryButtonStyle(enabled: canSave))
                        .disabled(saving || !canSave)
                }

                if saving {
                    ProgressView().controlSize(.small)
                }
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 12)
    }

    private var canAdvance: Bool {
        switch step {
        case .accounts: return !(draft?.accountIds.isEmpty ?? true)
        case .schedule: return !(draft?.weekdays.isEmpty ?? true)
        case .preview: return true
        }
    }

    private var canSave: Bool {
        guard let draft else { return false }
        return draft.isValid
    }

    // MARK: - 动作

    @MainActor
    private func performSave(enable: Bool) async {
        guard let draft, let plan = status?.plan else { return }
        saving = true
        saveFailure = nil
        localHint = nil
        let isDirty = draft.isDirty(against: plan)

        let result = await RotationSaveFlow.run(
            dirty: isDirty,
            enable: enable,
            save: { await store.saveWorkbuddyRotation(draft.params, note: nil) },
            setEnabled: { await store.setWorkbuddyRotationEnabled($0) },
            reload: { await store.refreshWorkbuddyRotation(silent: true) }
        )

        saving = false
        switch result {
        case .savedOnly:
            localHint = "计划已保存。要生效还需要打开自动轮换。"
        case .enabled:
            leaveEditing()
        case .saveFailed:
            // 保存失败：保留表单内容，绝不用旧计划去开启。
            saveFailure = store.wbRotationNotice ?? "计划没有保存成功。"
        case .enableFailed:
            // 已经回读过服务端状态；这里只说明没有开启成功。
            saveFailure = store.wbRotationNotice.map { "开启失败：\($0)" } ?? "开启失败，计划仍是原来的状态。"
        }
    }

    private func handleFocus() {
        if focus == .recovery { showExecution = true }
        if focus == .setup, status != nil { beginEditing() }
    }

    private func beginEditing() {
        guard let plan = status?.plan else { return }
        draft = RotationDraft(plan: plan)
        step = initialStep
        saveFailure = nil
        localHint = nil
        editing = true
        tab = .plan
    }

    private func leaveEditing() {
        editing = false
        draft = nil
        step = .accounts
        saveFailure = nil
        localHint = nil
    }

    private func requestLeaveEditing() {
        guard editing, dirty else {
            leaveEditing()
            return
        }
        confirmDiscard = true
    }

    private func requestClose() {
        if editing, dirty {
            confirmDiscard = true
        } else {
            dismiss()
        }
    }

    private func toneColor(_ tone: SettingsTone) -> Color {
        switch tone {
        case .neutral: return ConsoleTheme.text
        case .ok: return ConsoleTheme.success
        case .warning: return ConsoleTheme.warning
        case .error: return ConsoleTheme.danger
        case .accent: return ConsoleTheme.accent
        }
    }
}
