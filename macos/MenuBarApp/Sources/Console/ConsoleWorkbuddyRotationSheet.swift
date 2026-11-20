import SwiftUI

/// 「夜间账号轮换」的设置与记录面板。
///
/// 刻意**不**展示状态机名字、锁、JSON、RPC —— 只回答用户真正关心的四件事：
/// 什么时候跑、跑哪些账号、现在在干什么、原账号回来了没有。
struct ConsoleWorkbuddyRotationSheet: View {
    @ObservedObject var store: ConsoleStore
    @Environment(\.dismiss) private var dismiss

    @State private var weekdays: Set<Int> = []
    @State private var startMinutes = 22 * 60 + 30
    @State private var endMinutes = 7 * 60
    @State private var accountIds: [String] = []
    @State private var dwellSeconds = 3600
    @State private var timezone = "Asia/Shanghai"
    @State private var loaded = false
    @State private var confirmRunNow = false
    @State private var confirmRestore = false
    @State private var showHistory = false
    /// 执行记录的结果筛选：`all` / `done` / `skipped` / `failed`。
    @State private var historyFilter = "all"

    private let dwellChoices = [300, 600, 900, 1800, 2700, 3600, 5400, 7200]
    private let weekdayNames = ["一", "二", "三", "四", "五", "六", "日"]

    /// 离屏渲染时需要直接展开执行记录；正常打开保持收起。
    /// 离屏渲染用：截图不能滚动，所以允许只画其中一段。
    enum Focus { case all, history, status }

    init(
        store: ConsoleStore,
        expandedHistory: Bool = false,
        focus: Focus = .all
    ) {
        self.store = store
        _showHistory = State(initialValue: expandedHistory)
        self.focus = focus
    }

    private let focus: Focus

    private var status: WorkbuddyRotationStatus? { store.wbRotationStatus }

    private var cnAccounts: [ConsoleAccount] {
        store.accounts.filter { $0.isWorkbuddy(international: false) }
    }

    private var dirty: Bool {
        guard let plan = status?.plan else { return false }
        return Set(plan.weekdays) != weekdays
            || plan.startMinutes != startMinutes
            || plan.endMinutes != endMinutes
            || plan.accountIds != accountIds
            || plan.dwellSeconds != dwellSeconds
            || plan.timezone != timezone
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            ScrollView {
                switch focus {
                case .history:
                    historyBlock.padding(20)
                case .status:
                    VStack(alignment: .leading, spacing: 18) {
                        statusBlock
                        actionBlock
                    }
                    .padding(20)
                case .all:
                    VStack(alignment: .leading, spacing: 18) {
                        enableRow
                        scheduleBlock
                        accountBlock
                        statusBlock
                        actionBlock
                        historyBlock
                    }
                    .padding(20)
                }
            }
        }
        .frame(width: 560, height: 660)
        .background(ConsoleTheme.canvas)
        .onAppear(perform: syncFromStore)
        .onChange(of: store.wbRotationStatus) { _, _ in
            // 只有用户没在编辑时才覆盖表单，避免刷新把正在改的值冲掉。
            if !dirty { syncFromStore() }
        }
        .alert("立即执行一轮？", isPresented: $confirmRunNow) {
            Button("取消", role: .cancel) {}
            Button("开始") { Task { await store.runWorkbuddyRotationNow() } }
        } message: {
            Text("AICC 会立刻关闭并重启 WorkBuddy，按顺序切到你选的账号，并在窗口结束时切回**最初登录的那个**。"
                + "这会占用今天的自动轮换名额；建议只在你在场时用它做一次短时验证。")
        }
        .alert("恢复最初账号？", isPresented: $confirmRestore) {
            Button("取消", role: .cancel) {}
            Button("恢复") { Task { await store.restoreWorkbuddyRotationOrigin(force: true) } }
        } message: {
            Text("会用这一轮开始时保存的认证文件覆盖客户端当前登录态。"
                + "如果你刚刚手动换过账号，请不要继续。")
        }
    }

    // MARK: 头部

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text("夜间账号轮换")
                    .font(ConsoleType.sectionTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text("按计划依次切换 WorkBuddy 登录账号，窗口结束时回到这一轮最初的那个。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 0)
            Button("关闭") { dismiss() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
        }
        .padding(20)
    }

    private var enableRow: some View {
        VStack(alignment: .leading, spacing: 6) {
            Toggle(isOn: Binding(
                get: { status?.plan.enabled ?? false },
                set: { enabled in
                    Task {
                        // 开启才需要先把表单落盘（否则会用旧配置打开计划）；
                        // 关闭是独立动作，不能被一个无效表单挡住。
                        await RotationFeedbackText.applyToggle(
                            enabled: enabled,
                            dirty: dirty,
                            save: { await save(notify: nil) },
                            setEnabled: { await store.setWorkbuddyRotationEnabled($0) }
                        )
                    }
                }
            )) {
                Text("自动轮换")
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
            }
            .toggleStyle(.switch)

            Text("默认关闭。打开后，只有 AICC 正在运行时才会执行；退出或 Mac 休眠时不会「假装执行过」。")
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(12)
        .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(ConsoleTheme.border, lineWidth: 1)
        )
    }

    // MARK: 时间

    private var scheduleBlock: some View {
        VStack(alignment: .leading, spacing: 10) {
            sectionTitle("计划")
            HStack(spacing: 16) {
                DatePicker(
                    "开始",
                    selection: Binding(
                        get: { WorkbuddyTime.date(fromMinutes: startMinutes) },
                        set: { startMinutes = WorkbuddyTime.minutes(from: $0) }
                    ),
                    displayedComponents: .hourAndMinute
                )
                .frame(width: 150)

                DatePicker(
                    "结束",
                    selection: Binding(
                        get: { WorkbuddyTime.date(fromMinutes: endMinutes) },
                        set: { endMinutes = WorkbuddyTime.minutes(from: $0) }
                    ),
                    displayedComponents: .hourAndMinute
                )
                .frame(width: 150)
            }
            if endMinutes <= startMinutes {
                Text("结束时间早于开始时间：这是一个跨午夜的计划（例如 22:30 → 次日 07:00）。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }

            HStack(spacing: 6) {
                Text("星期")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                ForEach(1...7, id: \.self) { day in
                    Button {
                        if weekdays.contains(day) { weekdays.remove(day) } else { weekdays.insert(day) }
                    } label: {
                        Text(weekdayNames[day - 1])
                            .font(ConsoleType.caption)
                            .frame(width: 22, height: 20)
                    }
                    .buttonStyle(.plain)
                    .background(
                        weekdays.contains(day) ? ConsoleTheme.accentSoft : ConsoleTheme.card,
                        in: RoundedRectangle(cornerRadius: 5)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 5)
                            .strokeBorder(
                                weekdays.contains(day) ? ConsoleTheme.accent : ConsoleTheme.border,
                                lineWidth: 1
                            )
                    )
                    .foregroundStyle(weekdays.contains(day) ? ConsoleTheme.accent : ConsoleTheme.secondaryText)
                }
                Spacer(minLength: 0)
            }

            HStack(spacing: 10) {
                Text("每个账号停留")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Picker("", selection: $dwellSeconds) {
                    ForEach(dwellChoices, id: \.self) { choice in
                        Text(dwellLabel(choice)).tag(choice)
                    }
                }
                .labelsHidden()
                .frame(width: 110)

                Text("时区 \(timezone)")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Text("（在「设置 → WorkBuddy 客户端」修改）")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Spacer(minLength: 0)
            }
            if let status {
                Text("固定规则：错过开始时间 \(max(1, status.lateStartToleranceSeconds / 60)) 分钟以上不补跑；"
                    + "每次切换前预留 \(max(1, status.restoreReserveSeconds / 60)) 分钟恢复余量。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    // MARK: 账号

    private var accountBlock: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("参与账号", detail: accountIds.isEmpty ? "至少选一个" : "共 \(accountIds.count) 个，按下面的顺序切换")
            if cnAccounts.isEmpty {
                Text("还没有 WorkBuddy 国内版账号。先添加账号并完成授权。")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            } else {
                VStack(spacing: 0) {
                    ForEach(cnAccounts) { account in
                        HStack(spacing: 8) {
                            Toggle(isOn: Binding(
                                get: { accountIds.contains(account.id) },
                                set: { on in toggle(account.id, on: on) }
                            )) {
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(account.nickname)
                                        .font(ConsoleType.small)
                                        .foregroundStyle(ConsoleTheme.text)
                                    Text("\(WorkbuddyLiveLogin.shortUID(account.uid)) · \(account.displayAuthLabel)")
                                        .font(ConsoleType.caption)
                                        .foregroundStyle(ConsoleTheme.tertiaryText)
                                }
                            }
                            .toggleStyle(.checkbox)

                            Spacer(minLength: 0)
                            if let index = accountIds.firstIndex(of: account.id) {
                                Text("第 \(index + 1) 位")
                                    .font(ConsoleType.caption)
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                                Button {
                                    move(account.id, by: -1)
                                } label: {
                                    Image(systemName: "arrow.up")
                                }
                                .buttonStyle(.plain)
                                .disabled(index == 0)
                                Button {
                                    move(account.id, by: 1)
                                } label: {
                                    Image(systemName: "arrow.down")
                                }
                                .buttonStyle(.plain)
                                .disabled(index == accountIds.count - 1)
                            }
                        }
                        .padding(.vertical, 5)
                        Divider().overlay(ConsoleTheme.border)
                    }
                }
            }
        }
    }

    // MARK: 状态

    @ViewBuilder
    private var statusBlock: some View {
        if let status {
            VStack(alignment: .leading, spacing: 8) {
                sectionTitle("当前状态")
                row("客户端登录", status.liveLoggedIn
                    ? (status.liveUid.map(WorkbuddyLiveLogin.shortUID) ?? "未知")
                    : "未登录")
                row("下一轮", status.schedule.inWindow ? "正在执行窗口内" : (status.schedule.nextRunAt.map(WorkbuddyTime.dayTime) ?? "没有下一次"))
                if let current = status.current {
                    row("这一轮", "\(current.stateText) · 最初 \(current.originUidShort ?? "—")")
                    row("原账号", current.restoreText)
                } else if let last = status.lastRun {
                    row("上一轮", "\(last.stateText) · \(last.restoreText)")
                }
                if let notice = store.wbRotationNotice {
                    Text(notice)
                        .font(ConsoleType.caption)
                        .foregroundStyle(store.wbRotationFailed ? ConsoleTheme.danger : ConsoleTheme.secondaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let error = store.wbRotationError {
                    Text("读不到轮换状态：\(error)")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.danger)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if status.attentionRun != nil {
                    Text("自动恢复没有完成。请先确认客户端现在登录的是谁，再决定是否点「恢复最初账号」。")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.warning)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(12)
            .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 10))
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )
        }
    }

    // MARK: 动作

    private var actionBlock: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Button(dirty ? "保存计划" : "已保存") {
                    Task { _ = await save(notify: "计划已保存。") }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(!dirty)

                Button("预演") { Task { await store.previewWorkbuddyRotation() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(!RotationFeedbackText.canPreview(
                        dirty: dirty, busy: store.wbRotationBusy
                    ))
                    .help("只演算会切哪些账号、会跳过哪些，不修改任何登录态")

                Button("立即执行一轮") { confirmRunNow = true }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(!RotationFeedbackText.canRunNow(
                        dirty: dirty, busy: store.wbRotationBusy
                    ))
                    .help("会关闭并重启客户端；用于你在场时的短时验证")

                Spacer(minLength: 0)
            }
            HStack(spacing: 8) {
                Button("暂停并恢复原账号") {
                    Task { await store.stopWorkbuddyRotation() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.wbRotationBusy || (status?.current?.isTerminal ?? true))

                // 恢复失败时 `current` 是 nil（后端只把它留在 lastRun），
                // 所以这里也要认 attentionRun，否则"去处理"进去后按钮是灰的。
                Button("恢复最初账号…") { confirmRestore = true }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(store.wbRotationBusy)
                    .help("用这一轮开始时保存的快照覆盖客户端登录态")

                if store.wbRotationBusy {
                    ProgressView().controlSize(.mini)
                }
                Spacer(minLength: 0)
            }

            if dirty {
                Text("有未保存的修改：先保存，预演和立即执行才会按新计划演算。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.warning)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if let dry = store.wbRotationDryRun {
                VStack(alignment: .leading, spacing: 4) {
                    Text("预演结果")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.text)
                    ForEach(dry.steps) { step in
                        Text("· " + step.summary)
                            .font(ConsoleType.caption)
                            .foregroundStyle(step.outcome == "switch" ? ConsoleTheme.secondaryText : ConsoleTheme.tertiaryText)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    ForEach(dry.warnings, id: \.self) { warning in
                        Text("· " + warning)
                            .font(ConsoleType.caption)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(10)
                .background(ConsoleTheme.card.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    // MARK: 记录

    private var historyBlock: some View {
        VStack(alignment: .leading, spacing: 8) {
            DisclosureGroup(isExpanded: $showHistory) {
                VStack(alignment: .leading, spacing: 10) {
                    Picker("", selection: $historyFilter) {
                        Text("全部").tag("all")
                        Text("成功").tag("done")
                        Text("跳过").tag("skipped")
                        Text("失败").tag("failed")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 260)

                    if filteredRuns.isEmpty {
                        Text(status?.history.isEmpty == true ? "还没有执行记录。" : "没有符合条件的记录。")
                            .font(ConsoleType.caption)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    } else {
                        historyHeader
                        ForEach(filteredRuns) { run in
                            historyRun(run)
                        }
                    }
                }
                .padding(.top, 8)
            } label: {
                Text("执行记录")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
        }
    }

    /// 四列：日期 / 账号 / 切换结果 / 活跃。切换成功与活跃确认分开显示。
    private var historyHeader: some View {
        HStack(spacing: 8) {
            Text("日期")
                .frame(width: 74, alignment: .leading)
            Text("账号")
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("切换")
                .frame(width: 68, alignment: .leading)
            Text("活跃")
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(ConsoleType.caption)
        .foregroundStyle(ConsoleTheme.tertiaryText)
    }

    @ViewBuilder
    private func historyRun(_ run: WorkbuddyRotationRun) -> some View {
        let steps = run.steps.filter { matchesFilter($0) }
        VStack(alignment: .leading, spacing: 3) {
            // 轮次头放在步骤**上面**：恢复摘要属于这一轮，不该漂到下一轮的行里。
            HStack(spacing: 6) {
                Text("本轮恢复：\(run.restoreText)")
                Text(run.triggerText)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Spacer(minLength: 0)
                Text(run.stateText)
                    .foregroundStyle(run.needsAttention ? ConsoleTheme.warning : ConsoleTheme.tertiaryText)
            }
            .font(ConsoleType.caption)
            .foregroundStyle(ConsoleTheme.secondaryText)
            .padding(.bottom, 1)

            ForEach(steps) { step in
                HStack(spacing: 8) {
                    Text(run.planDate)
                        .frame(width: 74, alignment: .leading)
                    Text(nickname(step.accountId))
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text(step.statusText)
                        .foregroundStyle(step.status == "failed" ? ConsoleTheme.danger : ConsoleTheme.secondaryText)
                        .frame(width: 68, alignment: .leading)
                    Text(step.activityText)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .help(step.skipText ?? step.detail ?? "")
            }
            if let detail = run.detail, !detail.isEmpty {
                Text(detail)
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 4)
        Divider().overlay(ConsoleTheme.border)
    }

    private var filteredRuns: [WorkbuddyRotationRun] {
        (status?.history ?? []).filter { run in
            run.steps.isEmpty ? matchesFilter(run) : run.steps.contains(where: matchesFilter)
        }
    }

    private func matchesFilter(_ step: WorkbuddyRotationStep) -> Bool {
        switch historyFilter {
        case "done": return step.status == "done" || step.status == "dwelling"
        case "skipped": return step.status == "skipped" || step.status == "already_live"
        case "failed": return step.status == "failed"
        default: return true
        }
    }

    private func matchesFilter(_ run: WorkbuddyRotationRun) -> Bool {
        switch historyFilter {
        case "done": return run.state == "completed"
        case "skipped": return run.state == "skipped"
        case "failed": return run.state == "failed" || run.state == "recovery_required"
        default: return true
        }
    }

    // MARK: 辅助

    private func sectionTitle(_ title: String, detail: String? = nil) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.text)
            if let detail {
                Text(detail)
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
            Spacer(minLength: 0)
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .frame(width: 68, alignment: .leading)
            Text(value)
                .font(ConsoleType.caption)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
    }

    private func dwellLabel(_ seconds: Int) -> String {
        seconds % 3600 == 0 ? "\(seconds / 3600) 小时" : "\(seconds / 60) 分钟"
    }

    private func nickname(_ accountId: String) -> String {
        store.accounts.first(where: { $0.id == accountId })?.nickname ?? String(accountId.prefix(6))
    }

    private func toggle(_ accountId: String, on: Bool) {
        if on {
            if !accountIds.contains(accountId) { accountIds.append(accountId) }
        } else {
            accountIds.removeAll { $0 == accountId }
        }
    }

    private func move(_ accountId: String, by offset: Int) {
        guard let index = accountIds.firstIndex(of: accountId) else { return }
        let target = index + offset
        guard accountIds.indices.contains(target) else { return }
        accountIds.swapAt(index, target)
    }

    private func syncFromStore() {
        guard let plan = status?.plan else { return }
        weekdays = Set(plan.weekdays)
        startMinutes = plan.startMinutes
        endMinutes = plan.endMinutes
        accountIds = plan.accountIds
        dwellSeconds = plan.dwellSeconds
        timezone = plan.timezone
        loaded = true
    }

    /// 保存表单。返回 `false` = 没保存成功（调用方必须停下）。
    private func save(notify: String?) async -> Bool {
        await store.saveWorkbuddyRotation([
            "weekdays": weekdays.sorted(),
            "startMinutes": startMinutes,
            "endMinutes": endMinutes,
            "accountIds": accountIds,
            "dwellSeconds": dwellSeconds,
            "timezone": timezone,
        ], note: notify)
    }
}
