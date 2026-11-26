import SwiftUI

/// 自动轮换管理：日常只看计划与结果，改配置才进分步编辑。
///
/// 与旧弹层的区别不是尺寸，而是**职责**：
/// - 打开默认是概览（开没开、几个账号、什么时候跑、上次怎样、要不要处理）；
/// - 概览里直接有开启/关闭；关闭不要求先保存表单；
/// - 配置走三步（账号 → 时间 → 预演/保存），一次设置好；
/// - 执行操作（立即执行 / 停止 / 恢复）单独收在「执行」区，不和保存并排。
///
/// 所有"该不该问、能不能发"的判断在 `RotationManagerModel`，视图只画。
struct RotationManagerView: View {
    @ObservedObject var store: ConsoleStore
    @StateObject private var model: RotationManagerModel
    let focus: RotationManagerFocus

    @Environment(\.dismiss) private var dismiss

    @State private var showExecution = false
    @State private var confirmRunNow = false
    @State private var confirmRestore = false

    /// 离屏渲染：直接呈现保存/启用失败。生产构建里没有这个参数。
    #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
    var visualSaveFailure: String?
    #endif

    init(
        store: ConsoleStore,
        focus: RotationManagerFocus = .plan,
        initialStep: RotationSetupStep = .accounts,
        seedDraft: RotationDraft? = nil
    ) {
        self.store = store
        self.focus = focus
        _model = StateObject(wrappedValue: RotationManagerModel(
            store: store,
            focus: focus,
            initialStep: initialStep,
            seedDraft: seedDraft
        ))
        _showExecution = State(initialValue: focus == .recovery)
    }

    private var overview: RotationOverview { model.overview }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            tabBar
            Divider().overlay(ConsoleTheme.border)

            ScrollView {
                VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                    if model.tab == .plan {
                        if model.editing {
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

            if model.editing && model.tab == .plan {
                Divider().overlay(ConsoleTheme.border)
                setupFooter
            }
        }
        .frame(width: 760, height: 620)
        .background(ConsoleTheme.canvas)
        .onAppear {
            model.handleFocus()
            if focus == .recovery { showExecution = true }
            #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
            if let visualSaveFailure, model.editing { model.applyVisualFailure(visualSaveFailure) }
            #endif
        }
        .onChange(of: model.closeRequested) { _, requested in
            if requested { dismiss() }
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
        .alert("开启自动轮换？", isPresented: firstEnableBinding) {
            Button("取消", role: .cancel) { model.cancelFirstEnable() }
            Button("开启") { Task { await model.confirmFirstEnable() } }
        } message: {
            Text("开启后，计划会在窗口内关闭并重启 WorkBuddy 来切换账号，可能影响你正在使用的工作。\n"
                + "AICC 没有运行、或 Mac 处于休眠时，不能保证计划执行。\n"
                + "每轮结束会按既有规则尝试恢复最初登录的账号；恢复失败时需要你手动处理。")
        }
        .confirmationDialog("放弃未保存的修改？", isPresented: discardBinding) {
            Button("放弃修改", role: .destructive) { model.confirmLeave() }
            Button("继续编辑", role: .cancel) { model.cancelLeave() }
        } message: {
            Text("账号顺序与时间还没有保存，离开后这些修改会丢失。")
        }
    }

    // MARK: - 弹层绑定

    /// 只读绑定：关闭由按钮动作负责。
    ///
    /// 如果在这里的 `set(false)` 里取消，SwiftUI 会在调用按钮动作**之前**
    /// 先把待办清掉，"放弃修改/开启"就会变成空操作。
    private var discardBinding: Binding<Bool> {
        Binding(get: { model.pendingLeave != nil }, set: { _ in })
    }

    private var firstEnableBinding: Binding<Bool> {
        Binding(get: { model.pendingEnable != nil }, set: { _ in })
    }

    // MARK: - 头部与标签

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("自动轮换")
                .font(ConsoleType.detailTitle)
                .foregroundStyle(ConsoleTheme.text)
            Spacer(minLength: 0)
            Button("关闭") { model.requestClose() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .keyboardShortcut(.cancelAction)
                .disabled(model.saving)
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 14)
    }

    private var tabBar: some View {
        HStack(spacing: 8) {
            Picker("", selection: Binding(
                get: { model.tab },
                set: { model.requestTab($0) }
            )) {
                ForEach(RotationManagerTab.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 200)
            .disabled(model.saving)

            Spacer(minLength: 0)

            if model.editing {
                Text("第 \(model.step.rawValue + 1)/3 步 · \(model.step.title)")
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

            if let reason = overview.staleReason {
                // 有缓存但这次没读到：旧值可以看，但不能当成当前状态。
                ConsoleInlineNotice(
                    kind: .warning,
                    message: "最新一次读取失败：\(reason)。下面显示的是上次读到的状态。",
                    actionTitle: "重试"
                ) {
                    Task { await store.refreshWorkbuddyRotation() }
                }
            }

            if let error = store.wbRotationError, model.status == nil {
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
                // 正常开/关由"自动执行"一行说清；只有执行中、待处理、
                // 读取失败这些额外事实才值得再占一行。
                if showsStateRow {
                    overviewRow("状态", value: overview.state.statusText, tone: overview.state.tone)
                    rowDivider
                }
                autoRunRow
                rowDivider
                if overview.configured {
                    overviewRow("参与账号", value: "\(overview.accountCount) 个")
                    rowDivider
                    overviewRow("计划时间", value: overview.scheduleText)
                    if let next = overview.nextRunText, overview.staleReason == nil {
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

    private var showsStateRow: Bool {
        if overview.staleReason != nil { return true }
        switch overview.state {
        case .on, .off: return false
        default: return true
        }
    }

    /// 概览里的开/关。关闭是安全方向，不需要确认，也不先保存表单。
    private var autoRunRow: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text("自动执行")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 76, alignment: .leading)

            if model.status == nil || overview.staleReason != nil {
                // 读不到或只有缓存：不给开关，也不假装"未配置"。
                Text("无法确认")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Spacer(minLength: 0)
            } else if !overview.configured {
                // 确实还没配置过：这里不摆一个永远点不动的开关。
                Text("未配置")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Spacer(minLength: 0)
            } else if let enabled = model.planEnabled {
                Text(enabled ? "已开启" : "未开启")
                    .font(ConsoleType.body)
                    .foregroundStyle(enabled ? ConsoleTheme.success : ConsoleTheme.secondaryText)
                Spacer(minLength: 8)
                if model.togglingPlan {
                    ProgressView().controlSize(.small)
                }
                Button(enabled ? "关闭" : "开启…") {
                    model.requestPlanEnabled(!enabled)
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(model.togglingPlan
                          || (!enabled && !overview.configured)
                          || (!enabled && model.status?.current?.isTerminal == false))
                .help(enabled
                      ? "停止后续计划；正在执行的一轮会安全收尾并恢复原账号。"
                      : (model.status?.current?.isTerminal == false
                         ? "这一轮还在收尾，结束后再开启。"
                         : "先确认风险再开启。"))
            } else {
                Text("无法确认")
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Spacer(minLength: 0)
            }
        }
        .padding(.vertical, 9)
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
            Button(overview.configured ? "设置计划…" : "设置轮换计划…") { model.beginEditing() }
                .buttonStyle(ConsolePrimaryButtonStyle(enabled: model.status != nil))
                .disabled(model.status == nil)
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
                            disabled: store.wbRotationBusy || model.status?.current?.isTerminal != false
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
                Text(overview.planEnabled ? "上一轮没有完成恢复" : "上一轮没有完成恢复（计划已关闭）")
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
        if let draft = model.draft, model.plan != nil {
            RotationSetupView(
                store: store,
                draft: Binding(
                    get: { model.draft ?? draft },
                    set: { model.replaceDraft($0) }
                ),
                step: model.step,
                preview: model.preview,
                onPreview: { Task { await model.previewSavedPlan() } }
            )
        } else if let error = store.wbRotationError, model.status == nil {
            ConsoleInlineNotice(kind: .error, message: "读不到轮换计划：\(error)", actionTitle: "重试") {
                Task { await store.refreshWorkbuddyRotation() }
            }
        } else {
            ConsoleLoadingRow(text: "正在读取轮换计划…")
        }
    }

    private var setupFooter: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let saveFailure = model.saveFailure {
                ConsoleInlineNotice(kind: .error, message: saveFailure)
            } else if let localHint = model.localHint {
                ConsoleInlineNotice(kind: .info, message: localHint)
            } else if model.dirty {
                Text("有未保存的修改")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.warning)
            }

            HStack(spacing: 8) {
                Button("取消") { model.requestLeave(.cancelEditing) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(model.saving)

                if model.planEnabled == true {
                    // 关闭不受表单状态影响：这里不保存、不校验草稿。
                    Button("关闭自动轮换") { model.requestPlanEnabled(false) }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(model.saving || model.togglingPlan)
                        .help("停止后续计划。不会保存当前表单，也不会校验它。")
                }

                Spacer(minLength: 8)

                if model.step.previous != nil {
                    Button("上一步") { model.step = model.step.previous ?? .accounts }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(model.saving)
                }

                if let next = model.step.next {
                    Button("下一步") { model.step = next }
                        .buttonStyle(ConsolePrimaryButtonStyle(enabled: canAdvance))
                        .disabled(!canAdvance || model.saving)
                } else {
                    Button("仅保存") { model.requestSaveOnly() }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        // 没有改动时"已保存"是句空话，不如直接不给点。
                        .disabled(model.saving || !canSave || !model.dirty)
                    Button("保存并开启") { model.requestSaveAndEnable() }
                        .buttonStyle(ConsolePrimaryButtonStyle(enabled: canSave))
                        .disabled(model.saving || !canSave)
                }

                if model.saving {
                    ProgressView().controlSize(.small)
                }
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 12)
    }

    private var canAdvance: Bool {
        switch model.step {
        case .accounts: return !(model.draft?.accountIds.isEmpty ?? true)
        case .schedule: return !(model.draft?.weekdays.isEmpty ?? true)
        case .preview: return true
        }
    }

    private var canSave: Bool {
        model.draft?.isValid ?? false
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
