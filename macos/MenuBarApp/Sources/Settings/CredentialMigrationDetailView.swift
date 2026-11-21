import SwiftUI

/// CRED-4 / CRED-5：凭据管理详情。
///
/// 首屏只回答三件事：有没有要处理的、点哪个按钮、上一次做了什么。
/// 盘点明细、逐条状态、备份路径都在「更多操作」里，默认收起。
///
/// 三条产品约束一条没少：
/// 1. **不自动迁移**：只有用户点按钮才会读钥匙串（会弹系统授权框）；
/// 2. **迁移前备份**：确认框写明"先备份、备份不含令牌"；
/// 3. **核验成功前不删原始凭据**：所以永远有"回滚"，且回滚不依赖备份文件。
struct CredentialMigrationDetailView: View {
    @ObservedObject var worker: AccountWorkerManager
    let onClose: () -> Void
    /// 弹层高度。生产固定 520；视觉回归用更高的窗口一次看全展开内容。
    var height: CGFloat = 520
    /// 离屏渲染不读真实盘点。生产构建里没有这个参数。
    #if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
    var autoRefresh: Bool = true
    var initialShowMore: Bool = false
    #endif

    @State private var confirmMigrate = false
    @State private var confirmRollback = false
    @State private var confirmRelink = false
    @State private var confirmKeyRetry = false
    @State private var showMore = false

    @Environment(\.dismiss) private var dismiss

    private var state: CredentialMigrationState? { worker.credentialMigration }
    private var summary: CredentialMigrationSummary { state?.summary ?? .empty }
    private var verify: CredentialVerifyReport? { worker.credentialVerifyReport }

    private var relinkCount: Int { summary.needsRelink }
    private var keyFailedCount: Int { state?.keyMigration.failed ?? 0 }
    private var blockedCount: Int { summary.blocked }
    private var hasIssues: Bool { relinkCount + keyFailedCount + blockedCount > 0 }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()

            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                        summaryLine
                        if hasIssues { issuesGroup }
                        routineGroup
                        moreGroup.id("more")
                    }
                    .padding(.horizontal, ConsoleTheme.Sheet.padding)
                    .padding(.vertical, 16)
                    .frame(maxWidth: 560, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .center)
                }
                .onChange(of: showMore) { _, expanded in
                    guard expanded else { return }
                    withAnimation(.easeOut(duration: 0.18)) {
                        proxy.scrollTo("more", anchor: .top)
                    }
                }
            }

            if let notice = worker.credentialMigrationNotice {
                Divider()
                HStack(spacing: 8) {
                    Image(systemName: "info.circle")
                        .font(ConsoleType.caption)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                    Text(SettingsNoticeText.brief(notice))
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .lineLimit(2)
                    Spacer(minLength: 8)
                    if worker.credentialMigrationBusy {
                        ProgressView().controlSize(.small)
                    }
                }
                .padding(.horizontal, ConsoleTheme.Sheet.padding)
                .padding(.top, 10)
                .padding(.bottom, 14)
            }
        }
        .background(ConsoleTheme.canvas)
        .frame(width: 560, height: height)
        #if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
        .onAppear { if initialShowMore { showMore = true } }
        #endif
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
            Button("先备份并迁移") { Task { await worker.migrateCredentials() } }
            Button("取消", role: .cancel) {}
        } message: {
            Text("会先备份数据库与清单（不含令牌）。原始钥匙串条目不会删除，可以随时回滚。")
        }
        .confirmationDialog("把引用改回钥匙串？", isPresented: $confirmRollback) {
            Button("回滚到钥匙串") { Task { await worker.rollbackCredentialMigration() } }
            Button("取消", role: .cancel) {}
        } message: {
            Text("只回滚本次迁移产生、之后没有被重新授权的副本。副本删除前会核对引用；凭据重新从钥匙串读取（可能触发系统授权框）。")
        }
        .confirmationDialog("把借用授权的引用改成 Magpie 文件？", isPresented: $confirmRelink) {
            Button("重新关联（不复制令牌）") { Task { await worker.relinkBorrowedCredentials() } }
            Button("取消", role: .cancel) {}
        } message: {
            Text("只改引用，不读取、不复制、不删除任何令牌；旧钥匙串条目原样保留。需要 Magpie 已登录且该账号能唯一匹配。")
        }
        .confirmationDialog("重试会读一次旧钥匙串条目", isPresented: $confirmKeyRetry) {
            Button("重试旧键搬运") { Task { await worker.retryKeychainKeyMigration() } }
            Button("取消", role: .cancel) {}
        } message: {
            Text("系统可能弹出授权框。若之前点过「不允许」，请这次选择允许；仍然失败就保持原样，不影响已有授权。")
        }
    }

    // MARK: - 头

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text("管理凭据")
                .font(ConsoleType.detailTitle)
                .foregroundStyle(ConsoleTheme.text)
            Spacer()
            Button("关闭") { close() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, ConsoleTheme.Sheet.padding)
        .padding(.vertical, 14)
    }

    private var summaryLine: some View {
        HStack(spacing: 6) {
            Text("仍在钥匙串 \(summary.migratable) 条")
                .foregroundStyle(ConsoleTheme.secondaryText)
            if summary.rollbackable > 0 {
                Text("·").foregroundStyle(ConsoleTheme.tertiaryText)
                Text("可回滚 \(summary.rollbackable) 条")
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            if hasIssues {
                Text("·").foregroundStyle(ConsoleTheme.tertiaryText)
                Text("\(relinkCount + keyFailedCount + blockedCount) 项需处理")
                    .foregroundStyle(blockedCount > 0 || keyFailedCount > 0
                                     ? ConsoleTheme.danger
                                     : ConsoleTheme.warning)
            }
            Spacer(minLength: 0)
        }
        .font(ConsoleType.small)
    }

    // MARK: - 需要人工处理

    private var issuesGroup: some View {
        SettingsGroup(title: "需要处理") {
            if relinkCount > 0 {
                SettingsRow(
                    title: "借用授权仍指向钥匙串",
                    detail: "\(relinkCount) 条；不改会一直触发钥匙串访问。",
                    detailTone: .warning
                ) {
                    Button("重新关联…") { confirmRelink = true }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(worker.credentialMigrationBusy)
                }
                SettingsRowDivider()
            }

            if keyFailedCount > 0 {
                SettingsRow(
                    title: "旧键搬运失败",
                    detail: "\(keyFailedCount) 条；自动流程不会再试。",
                    detailTone: .error
                ) {
                    Button("重试…") { confirmKeyRetry = true }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(worker.credentialMigrationBusy)
                }
                SettingsRowDivider()
            }

            if blockedCount > 0 {
                SettingsRow(
                    title: "引用异常",
                    detail: "\(blockedCount) 条；引用不可用，需要重新授权。",
                    detailTone: .error
                ) {
                    EmptyView()
                }
            }
        }
    }

    // MARK: - 例行操作

    private var routineGroup: some View {
        SettingsGroup(title: "检查") {
            HStack(spacing: 8) {
                Button("刷新") { Task { await worker.refreshCredentialMigration() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(worker.credentialMigrationBusy)
                Button("核验") { Task { await worker.verifyCredentials() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(worker.credentialMigrationBusy)
                if worker.credentialMigrationBusy {
                    ProgressView().controlSize(.small)
                }
                Spacer(minLength: 0)
                if let verify {
                    Text(verifyLine(verify))
                        .font(ConsoleType.small)
                        .foregroundStyle(verify.hasProblems ? ConsoleTheme.danger : ConsoleTheme.secondaryText)
                }
            }
            .padding(.vertical, 8)
        }
    }

    // MARK: - 更多

    private var moreGroup: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button {
                showMore.toggle()
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: showMore ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                    Text("更多操作")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
            }
            .buttonStyle(.plain)

            if showMore {
                SettingsGroup(title: "迁移与回滚") {
                    SettingsRow(
                        title: "迁移全部",
                        detail: "把自有授权从钥匙串复制成本机 JSON（明文），原始条目保留。"
                    ) {
                        Button("迁移…") { confirmMigrate = true }
                            .buttonStyle(SettingsDestructiveButtonStyle())
                            .disabled(worker.credentialMigrationBusy || summary.migratable == 0)
                    }

                    SettingsRowDivider()

                    SettingsRow(
                        title: "回滚",
                        detail: "只回滚本次迁移产生、之后没有被重新授权的副本。"
                    ) {
                        Button("回滚…") { confirmRollback = true }
                            .buttonStyle(SettingsDestructiveButtonStyle())
                            .disabled(worker.credentialMigrationBusy || summary.rollbackable == 0)
                    }
                }

                inventoryGroup
                itemsGroup

                if let notice = worker.credentialMigrationNotice {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("完整结果")
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                        Text(notice)
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.vertical, 8)
                }
            }
        }
    }

    private var inventoryGroup: some View {
        SettingsGroup(title: "盘点") {
            SettingsValueRow(title: "已是本地 JSON", value: "\(summary.alreadyJSON) 条")
            SettingsRowDivider()
            SettingsValueRow(title: "已迁移（有记录）", value: "\(state?.migratedRecords ?? 0) 条")
            if summary.magpie > 0 {
                SettingsRowDivider()
                SettingsValueRow(title: "由 Magpie 管理", value: "\(summary.magpie) 条")
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
        }
    }

    private func close() {
        onClose()
        dismiss()
    }

    private func verifyLine(_ report: CredentialVerifyReport) -> String {
        var parts = ["正常 \(report.ok)"]
        if report.broken > 0 { parts.append("异常 \(report.broken)") }
        if report.orphanCount > 0 { parts.append("孤儿 \(report.orphanCount)") }
        if report.brokenFiles > 0 { parts.append("坏文件 \(report.brokenFiles)") }
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
