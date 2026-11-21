import SwiftUI

/// 自动计划：五类自动化一个入口看全，每行只回答"开没开、几个账号、什么时候跑、去哪管理"。
///
/// 只读：页面出现时最多补一次轮换状态（其它四个域由任务中心的首次加载负责），
/// 切换模式、点"管理"都只改导航，不发写请求。
struct ConsoleAutomationSection: View {
    @ObservedObject var store: ConsoleStore
    @EnvironmentObject private var nav: ConsoleNavigation

    private var inputs: AutomationInputs {
        AutomationInputs(
            checkinAuto: store.checkinAutoLoaded ? store.checkinAuto : nil,
            checkinLoading: store.checkinLoading,
            checkinError: store.checkinAutoError,
            growthAuto: store.growthAutoLoaded ? store.growthAuto : nil,
            growthLoading: store.growthLoading,
            growthError: store.growthAutoError,
            travelAuto: store.travelAuto,
            travelLoading: store.travelLoading,
            travelError: store.travelError,
            creditsAuto: store.qoderCreditsAuto,
            creditsLoading: store.qoderCreditsLoading,
            creditsError: store.qoderCreditsError,
            creditsLoaded: store.qoderCreditsLoaded,
            rotation: store.wbRotationStatus,
            rotationLoading: store.wbRotationLoading,
            rotationError: store.wbRotationError
        )
    }

    private var rows: [AutomationPlanRow] {
        AutomationPlanBuilder.rows(inputs)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            ConsoleListSurface {
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                        if index > 0 { Divider().overlay(ConsoleTheme.border) }
                        rowView(row)
                    }
                }
            }

            Text("每个计划各自调度，开关互不影响；AICC 退出后都不会执行。")
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.tertiaryText)
        }
        .task {
            // 只读：冷启动还没读过轮换状态时补一次，不触发任何写操作。
            if store.wbRotationStatus == nil && !store.wbRotationLoading {
                await store.refreshWorkbuddyRotation(silent: true)
            }
        }
    }

    private func rowView(_ row: AutomationPlanRow) -> some View {
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: row.kind.symbol)
                .font(.system(size: 12))
                .foregroundStyle(ConsoleTheme.secondaryText)
                .frame(width: 18)

            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(row.title)
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                        .lineLimit(1)
                    ConsoleStatusDot(color: toneColor(row.tone), text: row.statusText)
                }
                // 状态已经是"读取失败/读取中"时，摘要不再重复同一句话。
                if showsSummary(row) {
                    HStack(spacing: 6) {
                        Text(row.summary)
                            .lineLimit(1)
                        if let next = row.nextRun {
                            Text("·")
                            Text("下次 \(next)")
                                .monospacedDigit()
                        }
                    }
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            }

            Spacer(minLength: 8)

            if let message = failureMessage(row) {
                Text(message)
                    .font(ConsoleType.micro)
                    .foregroundStyle(ConsoleTheme.danger)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: 200, alignment: .trailing)
                    .help(message)
            }

            Button("管理") { manage(row.kind) }
                .buttonStyle(ConsoleSecondaryButtonStyle())
        }
        .padding(.vertical, 9)
    }

    private func showsSummary(_ row: AutomationPlanRow) -> Bool {
        !row.summary.isEmpty && row.summary != row.statusText
    }

    private func failureMessage(_ row: AutomationPlanRow) -> String? {
        if case .failed(let message) = row.state { return message }
        return nil
    }

    private func manage(_ kind: AutomationPlanKind) {
        switch kind {
        case .checkin:
            nav.openCheckin(platform: .workbuddy, workbuddy: .checkin, mode: .today)
        case .growth:
            nav.openCheckin(platform: .workbuddy, workbuddy: .growth, mode: .today)
        case .travel:
            nav.openCheckin(platform: .workbuddy, workbuddy: .travel, mode: .today)
        case .credits:
            nav.openCheckin(platform: .qoder, qoder: .credits, mode: .today)
        case .rotation:
            nav.requestRotationManager(.plan)
        }
    }

    private func toneColor(_ tone: SettingsTone) -> Color {
        switch tone {
        case .neutral: return ConsoleTheme.tertiaryText
        case .ok: return ConsoleTheme.success
        case .warning: return ConsoleTheme.warning
        case .error: return ConsoleTheme.danger
        case .accent: return ConsoleTheme.accent
        }
    }
}
