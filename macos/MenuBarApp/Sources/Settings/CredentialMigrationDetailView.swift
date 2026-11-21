import SwiftUI

/// CRED-4 / CRED-5：凭据管理详情。
///
/// 从设置主页搬出来，但**三条产品约束一条都没少**：
///
/// 1. **不自动迁移**：只有用户点按钮才会读钥匙串（读会弹系统授权框）；
/// 2. **迁移前备份**：按钮文案与确认框都写明"先备份、备份不含令牌"；
/// 3. **核验成功前不删原始凭据**：所以这里永远有"回滚"，且回滚不依赖备份文件。
///
/// 主页只留"当前方式 / 待处理数量 / 管理凭据入口"，明细在这里。
struct CredentialMigrationDetailView: View {
    @ObservedObject var worker: AccountWorkerManager
    let onClose: () -> Void
    /// 离屏渲染不读真实盘点。生产构建里没有这个参数。
    #if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
    var autoRefresh: Bool = true
    #endif

    @State private var confirmMigrate = false
    @State private var confirmRollback = false
    @State private var confirmRelink = false
    @State private var confirmKeyRetry = false
    @State private var showExplanation = false

    @Environment(\.dismiss) private var dismiss

    private var state: CredentialMigrationState? { worker.credentialMigration }
    private var summary: CredentialMigrationSummary { state?.summary ?? .empty }
    private var verify: CredentialVerifyReport? { worker.credentialVerifyReport }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                    explanation
                    summaryLine
                    actionGroup
                    overviewGroup
                    itemsGroup
                }
                .padding(.horizontal, ConsoleTheme.Sheet.padding)
                .padding(.vertical, 18)
                .frame(maxWidth: 640, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .center)
            }

            Divider()
            footer
        }
        .background(ConsoleTheme.canvas)
        .frame(width: 620, height: 620)
        .task {
            #if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
            guard autoRefresh else { return }
            #endif
            await worker.refreshCredentialMigration()
        }
        .confirmationDialog(
            "迁移后新写入的令牌会落成本机 JSON 文件（明文）",
            isPresented: $confirmMigrate
        ) {
            Button("先备份并迁移") {
                Task { await worker.migrateCredentials() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("会先备份数据库与清单（不含令牌）。原始钥匙串条目不会删除，可以随时回滚。")
        }
        .confirmationDialog("把引用改回钥匙串？", isPresented: $confirmRollback) {
            Button("回滚到钥匙串") {
                Task { await worker.rollbackCredentialMigration() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("只回滚本次迁移产生、之后没有被重新授权的副本。副本删除前会核对引用；凭据重新从钥匙串读取（可能触发系统授权框）。")
        }
        .confirmationDialog("把借用授权的引用改成 Magpie 文件？", isPresented: $confirmRelink) {
            Button("重新关联（不复制令牌）") {
                Task { await worker.relinkBorrowedCredentials() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("只改引用，不读取、不复制、不删除任何令牌；旧钥匙串条目原样保留。需要 Magpie 已登录且该账号能唯一匹配。")
        }
        .confirmationDialog("重试会读一次旧钥匙串条目", isPresented: $confirmKeyRetry) {
            Button("重试旧键搬运") {
                Task { await worker.retryKeychainKeyMigration() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("系统可能弹出授权框。若之前点过「不允许」，请这次选择允许；仍然失败就保持原样，不影响已有授权。")
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("管理凭据")
                    .font(ConsoleType.detailTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text("旧钥匙串授权的盘点、迁移、核验与回滚")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer()
            Button("关闭") { close() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, ConsoleTheme.Sheet.padding)
        .padding(.vertical, 14)
    }

    private var explanation: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                showExplanation.toggle()
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: showExplanation ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                    Text("这些操作会做什么")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
            }
            .buttonStyle(.plain)

            if showExplanation {
                VStack(alignment: .leading, spacing: 6) {
                    Text("迁移会把自有授权从钥匙串复制成本机 JSON 文件（明文）。迁移前会先备份数据库与清单（备份不含令牌），原始钥匙串条目一个都不删。")
                    Text("回滚只针对本次迁移产生的副本：之后重新登录过的授权不会被切回旧令牌；副本删不掉时记录会留着，下次回滚会重试。")
                    Text("借用授权（Magpie）不迁移：重新关联只把引用改成 Magpie 文件，不复制任何令牌。")
                }
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: ConsoleTheme.controlRadius))
    }

    /// 首屏一句摘要：先给结论，再给操作，明细在下面。
    private var summaryLine: some View {
        HStack(spacing: 6) {
            Text("仍在钥匙串 \(summary.migratable) 条")
                .foregroundStyle(ConsoleTheme.secondaryText)
            Text("·").foregroundStyle(ConsoleTheme.tertiaryText)
            Text("可回滚 \(summary.rollbackable) 条")
                .foregroundStyle(ConsoleTheme.secondaryText)
            if issues > 0 {
                Text("·").foregroundStyle(ConsoleTheme.tertiaryText)
                Text("需处理 \(issues) 项")
                    .foregroundStyle(attentionTone)
            }
            Spacer(minLength: 0)
        }
        .font(ConsoleType.small)
    }

    private var issues: Int {
        summary.blocked + summary.needsRelink + (state?.keyMigration.failed ?? 0)
    }

    private var attentionTone: Color {
        summary.blocked > 0 || (state?.keyMigration.failed ?? 0) > 0
            ? ConsoleTheme.danger
            : ConsoleTheme.warning
    }

    private var overviewGroup: some View {
        SettingsGroup(title: "盘点") {
            SettingsValueRow(
                title: "仍在钥匙串",
                value: "\(summary.migratable) 条",
                tone: summary.migratable > 0 ? .warning : .neutral
            )
            SettingsRowDivider()
            SettingsValueRow(title: "已是本地 JSON", value: "\(summary.alreadyJSON) 条")
            SettingsRowDivider()
            SettingsValueRow(title: "已迁移（有记录）", value: "\(state?.migratedRecords ?? 0) 条")
            if summary.magpie > 0 {
                SettingsRowDivider()
                SettingsValueRow(title: "由 Magpie 管理（不迁移）", value: "\(summary.magpie) 条")
            }
            if summary.blocked > 0 {
                SettingsRowDivider()
                SettingsValueRow(title: "引用异常", value: "\(summary.blocked) 条", tone: .error)
            }
            if summary.rollbackable > 0 {
                SettingsRowDivider()
                SettingsValueRow(title: "可回滚（本次迁移产生）", value: "\(summary.rollbackable) 条")
            }
            if let verify {
                SettingsRowDivider()
                SettingsValueRow(
                    title: "最近核验",
                    value: verifySummary(verify),
                    tone: verify.hasProblems ? .error : .neutral
                )
            }
        }
    }

    private var actionGroup: some View {
        SettingsGroup(
            title: "操作",
            footnote: "刷新与核验只读；迁移与回滚会改动凭据引用，按钮会先要求确认。"
        ) {
            if summary.needsRelink > 0 {
                SettingsRow(
                    title: "借用授权仍指向钥匙串",
                    detail: "\(summary.needsRelink) 条。不改会一直触发钥匙串访问。",
                    detailTone: .warning
                ) {
                    Button("重新关联…") { confirmRelink = true }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(worker.credentialMigrationBusy)
                }
                SettingsRowDivider()
            }

            if let failed = state?.keyMigration.failed, failed > 0 {
                SettingsRow(
                    title: "旧键搬运失败",
                    detail: "上次读钥匙串被拒绝或写入出错，自动流程不会再试。",
                    detailTone: .error
                ) {
                    Button("重试…") { confirmKeyRetry = true }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(worker.credentialMigrationBusy)
                }
                SettingsRowDivider()
            }

            HStack(spacing: 8) {
                Button("刷新") {
                    Task { await worker.refreshCredentialMigration() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(worker.credentialMigrationBusy)

                Button("核验") {
                    Task { await worker.verifyCredentials() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(worker.credentialMigrationBusy)

                Spacer(minLength: 8)

                if worker.credentialMigrationBusy {
                    ProgressView().controlSize(.small)
                }
            }
            .padding(.top, 8)
            .padding(.bottom, 6)

            SettingsRowDivider()

            HStack(spacing: 8) {
                Text("改动引用")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                Spacer(minLength: 8)

                Button("迁移全部…") { confirmMigrate = true }
                    .buttonStyle(SettingsDestructiveButtonStyle())
                    .disabled(worker.credentialMigrationBusy || summary.migratable == 0)

                Button("回滚…") { confirmRollback = true }
                    .buttonStyle(SettingsDestructiveButtonStyle())
                    .disabled(worker.credentialMigrationBusy || summary.rollbackable == 0)
            }
            .padding(.top, 6)
            .padding(.bottom, 8)

            if let notice = worker.credentialMigrationNotice {
                SettingsInlineNotice(kind: .info, message: notice)
                    .padding(.bottom, 8)
            }
        }
    }

    @ViewBuilder private var itemsGroup: some View {
        if let items = state?.items, !items.isEmpty {
            SettingsGroup(title: "逐条状态", detail: "\(items.count) 条") {
                ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                    if index > 0 { SettingsRowDivider() }
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.nickname.isEmpty ? item.accountId : item.nickname)
                                .font(ConsoleType.rowTitle)
                                .foregroundStyle(ConsoleTheme.text)
                                .lineLimit(1)
                            Text("\(item.domain) · \(item.source) · \(item.storageText)")
                                .font(ConsoleType.micro)
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 8)
                        Text(statusText(item))
                            .font(ConsoleType.small)
                            .foregroundStyle(statusColor(item))
                            .multilineTextAlignment(.trailing)
                            .lineLimit(2)
                    }
                    .padding(.vertical, 7)
                }
            }
        } else {
            ConsoleEmptyState(
                symbol: "key.slash",
                title: "还没有盘点结果",
                message: "点「刷新」读取引用与摘要。不会读取钥匙串里的令牌。"
            )
        }
    }

    private var footer: some View {
        HStack {
            Text("不会显示任何令牌内容。")
                .font(ConsoleType.micro)
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Spacer()
            Button("完成") { close() }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, ConsoleTheme.Sheet.padding)
        .padding(.vertical, 12)
    }

    private func close() {
        onClose()
        dismiss()
    }

    private func verifySummary(_ report: CredentialVerifyReport) -> String {
        var parts = ["正常 \(report.ok)"]
        if report.broken > 0 { parts.append("异常 \(report.broken)") }
        if report.needsRelink > 0 { parts.append("待重新关联 \(report.needsRelink)") }
        if report.pendingKeychain > 0 { parts.append("待迁移 \(report.pendingKeychain)") }
        if report.orphanCount > 0 { parts.append("孤儿文件 \(report.orphanCount)") }
        if report.brokenFiles > 0 { parts.append("坏文件 \(report.brokenFiles)") }
        if report.pendingCleanup > 0 { parts.append("待清理 \(report.pendingCleanup)") }
        return parts.joined(separator: " · ")
    }

    private func statusText(_ item: CredentialMigrationItem) -> String {
        if let blocked = item.blockedText { return blocked }
        return item.storageText
    }

    private func statusColor(_ item: CredentialMigrationItem) -> Color {
        if item.blocked == "needs_relink" { return ConsoleTheme.warning }
        if item.blocked != nil { return ConsoleTheme.secondaryText }
        return item.migratable ? ConsoleTheme.warning : ConsoleTheme.secondaryText
    }
}
