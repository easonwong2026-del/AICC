import SwiftUI

/// 执行记录：轮次日期、每个账号的切换结果、活跃确认、恢复结果。
///
/// 三个事实分开说：**切换成功**、**恢复成功**、**活跃确认**。
/// 切换成功不等于账号活跃，恢复成功也不等于这一轮没有跳过。
struct RotationHistoryView: View {
    @ObservedObject var store: ConsoleStore
    @State private var filter = "all"

    private var status: WorkbuddyRotationStatus? { store.wbRotationStatus }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Picker("", selection: $filter) {
                Text("全部").tag("all")
                Text("成功").tag("done")
                Text("跳过").tag("skipped")
                Text("失败").tag("failed")
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 260)

            if let error = store.wbRotationError, status == nil {
                ConsoleInlineNotice(kind: .error, message: "读不到执行记录：\(error)", actionTitle: "重试") {
                    Task { await store.refreshWorkbuddyRotation() }
                }
            } else if status == nil {
                ConsoleLoadingRow(text: "正在读取执行记录…")
            } else if filteredRuns.isEmpty {
                ConsoleEmptyState(
                    symbol: "clock.arrow.circlepath",
                    title: historyEmptyTitle,
                    message: "计划执行过后，每一轮的切换与恢复结果都会列在这里。"
                )
            } else {
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
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.tertiaryText)

                VStack(spacing: 0) {
                    ForEach(filteredRuns) { run in
                        runView(run)
                    }
                }
            }
        }
    }

    private var historyEmptyTitle: String {
        (status?.history.isEmpty ?? true) ? "还没有执行记录" : "没有符合条件的记录"
    }

    private func runView(_ run: WorkbuddyRotationRun) -> some View {
        let steps = run.steps.filter { matchesFilter($0) }
        return VStack(alignment: .leading, spacing: 3) {
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
        .padding(.vertical, 6)
        .overlay(alignment: .bottom) {
            Divider().overlay(ConsoleTheme.border)
        }
    }

    private var filteredRuns: [WorkbuddyRotationRun] {
        (status?.history ?? []).filter { run in
            run.steps.isEmpty ? matchesFilter(run) : run.steps.contains(where: matchesFilter)
        }
    }

    private func matchesFilter(_ step: WorkbuddyRotationStep) -> Bool {
        switch filter {
        case "done": return step.status == "done" || step.status == "dwelling"
        case "skipped": return step.status == "skipped" || step.status == "already_live"
        case "failed": return step.status == "failed"
        default: return true
        }
    }

    private func matchesFilter(_ run: WorkbuddyRotationRun) -> Bool {
        switch filter {
        case "done": return run.state == "completed"
        case "skipped": return run.state == "skipped"
        case "failed": return run.state == "failed" || run.state == "recovery_required"
        default: return true
        }
    }

    private func nickname(_ accountId: String) -> String {
        store.accounts.first(where: { $0.id == accountId })?.nickname ?? String(accountId.prefix(6))
    }
}
