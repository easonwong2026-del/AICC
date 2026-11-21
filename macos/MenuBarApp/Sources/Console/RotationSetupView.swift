import SwiftUI

/// 轮换计划的三步内容。底部操作栏在 `RotationManagerView` 里固定，
/// 账号再多也不用滚到列表底部才能保存。
struct RotationSetupView: View {
    @ObservedObject var store: ConsoleStore
    @Binding var draft: RotationDraft
    let step: RotationSetupStep
    let plan: WorkbuddyRotationPlan

    private var cnAccounts: [ConsoleAccount] {
        store.accounts.filter { $0.isWorkbuddy(international: false) }
    }

    var body: some View {
        switch step {
        case .accounts:
            accountsStep
        case .schedule:
            scheduleStep
        case .preview:
            previewStep
        }
    }

    // MARK: - 第一步：账号

    private var accountsStep: some View {
        VStack(alignment: .leading, spacing: 10) {
            if cnAccounts.isEmpty {
                ConsoleEmptyState(
                    symbol: "person.2.slash",
                    title: "还没有 WorkBuddy 国内版账号",
                    message: "先在「AI 账号」里添加账号并完成授权，这里才能选择。"
                )
            } else {
                ConsoleListSurface {
                    VStack(spacing: 0) {
                        ForEach(Array(cnAccounts.enumerated()), id: \.element.id) { index, account in
                            if index > 0 { Divider().overlay(ConsoleTheme.border) }
                            accountRow(account)
                        }
                    }
                }
                Text("只有 WorkBuddy 国内版账号能参与轮换；顺序就是切换顺序。授权失效的账号不能选，已选的可以取消。")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func accountRow(_ account: ConsoleAccount) -> some View {
        let selected = draft.accountIds.contains(account.id)
        let blocked = account.authTone == .error
        let order = draft.accountIds.firstIndex(of: account.id).map { $0 + 1 }

        return HStack(spacing: 10) {
            Button {
                draft.toggleAccount(account.id, on: !selected)
            } label: {
                Image(systemName: selected ? "checkmark.square.fill" : "square")
                    .font(.system(size: 13))
                    .foregroundStyle(selected ? ConsoleTheme.accent : ConsoleTheme.tertiaryText)
            }
            .buttonStyle(.plain)
            .disabled(blocked && !selected)

            VStack(alignment: .leading, spacing: 2) {
                Text(account.nickname)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                    .lineLimit(1)
                HStack(spacing: 6) {
                    Text(WorkbuddyLiveLogin.shortUID(account.uid))
                    Text("·")
                    Text(account.displayAuthLabel)
                    if blocked {
                        Text("· 不能参与：\(account.authLabel)，请先重新登录")
                            .foregroundStyle(ConsoleTheme.danger)
                    }
                }
                .font(ConsoleType.micro)
                .foregroundStyle(blocked ? ConsoleTheme.danger : ConsoleTheme.tertiaryText)
                .lineLimit(1)
            }

            Spacer(minLength: 8)

            if let order {
                Text("第 \(order) 位")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Button {
                    draft.moveAccount(account.id, by: -1)
                } label: {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 10))
                }
                .buttonStyle(ConsoleIconButtonStyle(size: 22))
                .disabled(order == 1)

                Button {
                    draft.moveAccount(account.id, by: 1)
                } label: {
                    Image(systemName: "arrow.down")
                        .font(.system(size: 10))
                }
                .buttonStyle(ConsoleIconButtonStyle(size: 22))
                .disabled(order == draft.accountIds.count)
            }
        }
        .padding(.vertical, 7)
    }

    // MARK: - 第二步：时间

    private var scheduleStep: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            ConsoleListSurface {
                VStack(spacing: 0) {
                    HStack(spacing: 16) {
                        Text("开始")
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                            .frame(width: 44, alignment: .leading)
                        DatePicker(
                            "开始",
                            selection: Binding(
                                get: { WorkbuddyTime.date(fromMinutes: draft.startMinutes) },
                                set: { draft.startMinutes = WorkbuddyTime.minutes(from: $0) }
                            ),
                            displayedComponents: .hourAndMinute
                        )
                        .labelsHidden()
                        .fixedSize()

                        Text("结束")
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                            .frame(width: 44, alignment: .leading)
                        DatePicker(
                            "结束",
                            selection: Binding(
                                get: { WorkbuddyTime.date(fromMinutes: draft.endMinutes) },
                                set: { draft.endMinutes = WorkbuddyTime.minutes(from: $0) }
                            ),
                            displayedComponents: .hourAndMinute
                        )
                        .labelsHidden()
                        .fixedSize()
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, 9)

                    Divider().overlay(ConsoleTheme.border)

                    weekdayRow

                    Divider().overlay(ConsoleTheme.border)

                    HStack(spacing: 12) {
                        Text("每账号停留")
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                        Spacer(minLength: 8)
                        Picker("每个账号停留", selection: $draft.dwellSeconds) {
                            ForEach(RotationDraft.dwellChoices, id: \.self) { choice in
                                Text(dwellLabel(choice)).tag(choice)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .fixedSize()
                    }
                    .padding(.vertical, 9)

                    Divider().overlay(ConsoleTheme.border)

                    HStack(spacing: 12) {
                        Text("时区")
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                        Spacer(minLength: 8)
                        Picker("时区", selection: $draft.timezone) {
                            ForEach(timezones, id: \.self) { zone in
                                Text(zone).tag(zone)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .fixedSize()
                    }
                    .padding(.vertical, 9)
                }
            }

            HStack(spacing: 6) {
                Text(draft.windowText)
                    .font(ConsoleType.body)
                    .foregroundStyle(ConsoleTheme.text)
                if draft.crossesMidnight {
                    Text("跨午夜")
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                Spacer(minLength: 0)
                Text("每账号 \(draft.dwellText)")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }

            if let status = store.wbRotationStatus {
                Text("错过开始 \(max(1, status.lateStartToleranceSeconds / 60)) 分钟以上本轮不补跑；"
                    + "每次切换前预留 \(max(1, status.restoreReserveSeconds / 60)) 分钟恢复余量。")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var weekdayRow: some View {
        HStack(spacing: 6) {
            Text("星期")
                .font(ConsoleType.rowTitle)
                .foregroundStyle(ConsoleTheme.text)
                .frame(width: 44, alignment: .leading)
            ForEach(1...7, id: \.self) { day in
                Button {
                    if draft.weekdays.contains(day) {
                        draft.weekdays.remove(day)
                    } else {
                        draft.weekdays.insert(day)
                    }
                } label: {
                    Text(AutomationScheduleText.name(day).replacingOccurrences(of: "周", with: ""))
                        .font(ConsoleType.caption)
                        .frame(width: 24, height: 22)
                }
                .buttonStyle(.plain)
                .background(
                    draft.weekdays.contains(day) ? ConsoleTheme.accentSoft : ConsoleTheme.card,
                    in: RoundedRectangle(cornerRadius: 5)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 5)
                        .strokeBorder(
                            draft.weekdays.contains(day) ? ConsoleTheme.accent : ConsoleTheme.border,
                            lineWidth: 1
                        )
                )
                .foregroundStyle(draft.weekdays.contains(day) ? ConsoleTheme.accent : ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 9)
    }

    private var timezones: [String] {
        let supported = store.wbRotationStatus?.supportedTimezones ?? []
        var values = supported.isEmpty ? ["Asia/Shanghai", "UTC"] : supported
        if !values.contains(draft.timezone) { values.append(draft.timezone) }
        return values
    }

    private func dwellLabel(_ seconds: Int) -> String {
        seconds % 3600 == 0 ? "\(seconds / 3600) 小时" : "\(seconds / 60) 分钟"
    }

    // MARK: - 第三步：预演

    private var previewStep: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            ConsoleListSurface {
                VStack(spacing: 0) {
                    summaryRow("参与账号", value: accountOrderText)
                    Divider().overlay(ConsoleTheme.border)
                    summaryRow("时间窗口", value: "\(AutomationScheduleText.weekdays(Array(draft.weekdays))) \(draft.windowText)")
                    Divider().overlay(ConsoleTheme.border)
                    summaryRow("每账号停留", value: draft.dwellText)
                    Divider().overlay(ConsoleTheme.border)
                    summaryRow("时区", value: draft.timezone)
                    Divider().overlay(ConsoleTheme.border)
                    summaryRow("结束动作", value: "切回这一轮最初登录的账号")
                }
            }

            HStack(spacing: 8) {
                Button("预演") { Task { await store.previewWorkbuddyRotation() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(store.wbRotationBusy)
                Text("只演算，不切换账号。")
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                Spacer(minLength: 0)
            }

            if let dry = store.wbRotationDryRun {
                ConsoleListSurface {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(dry.steps) { step in
                            Text("· " + step.summary)
                                .font(ConsoleType.small)
                                .foregroundStyle(step.outcome == "switch"
                                                 ? ConsoleTheme.secondaryText
                                                 : ConsoleTheme.tertiaryText)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        ForEach(dry.warnings, id: \.self) { warning in
                            Text("· " + warning)
                                .font(ConsoleType.small)
                                .foregroundStyle(ConsoleTheme.warning)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }

    private var accountOrderText: String {
        let names = draft.accountIds.map { id in
            store.accounts.first(where: { $0.id == id })?.nickname ?? String(id.prefix(6))
        }
        return names.isEmpty ? "至少选一个" : names.joined(separator: " → ")
    }

    private func summaryRow(_ label: String, value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(label)
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 80, alignment: .leading)
            Text(value)
                .font(ConsoleType.body)
                .foregroundStyle(ConsoleTheme.text)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 9)
    }
}
