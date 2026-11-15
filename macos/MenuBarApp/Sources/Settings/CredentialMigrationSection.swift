import SwiftUI

/// CRED-4：设置页里的「旧凭据迁移」面板。
///
/// 三条产品约束，界面必须自己体现出来，不能只写在文档里：
///
/// 1. **不自动迁移**：只有用户点按钮才会读钥匙串（读会弹系统授权框）；
/// 2. **迁移前备份**：按钮文案与确认框都写明"先备份、备份不含令牌"；
/// 3. **核验成功前不删原始凭据**：所以这里永远有"回滚"，且回滚不依赖备份文件。
struct CredentialMigrationSection: View {
    @ObservedObject var worker: AccountWorkerManager

    @State private var confirmMigrate = false
    @State private var confirmRollback = false
    @State private var confirmRelink = false

    private var state: CredentialMigrationState? { worker.credentialMigration }
    private var summary: CredentialMigrationSummary { state?.summary ?? .empty }
    private var verify: CredentialVerifyReport? { worker.credentialVerifyReport }

    var body: some View {
        Section("旧凭据迁移") {
            HStack {
                Text("仍在钥匙串")
                Spacer()
                Text("\(summary.migratable) 条")
                    .foregroundStyle(summary.migratable > 0 ? .orange : .secondary)
            }
            HStack {
                Text("已是本地 JSON")
                Spacer()
                Text("\(summary.alreadyJSON) 条").foregroundStyle(.secondary)
            }
            // "已迁移"只认迁移记录：原生新建的 JSON 授权不是迁移来的，混在一起会让
            // 用户以为迁移做过。
            HStack {
                Text("已迁移（有迁移记录）")
                Spacer()
                Text("\(state?.migratedRecords ?? 0) 条").foregroundStyle(.secondary)
            }
            if summary.magpie > 0 {
                HStack {
                    Text("由 Magpie 管理（不迁移）")
                    Spacer()
                    Text("\(summary.magpie) 条").foregroundStyle(.secondary)
                }
            }
            if summary.needsRelink > 0 {
                HStack {
                    Text("借用授权仍指向钥匙串")
                    Spacer()
                    Text("\(summary.needsRelink) 条").foregroundStyle(.orange)
                }
                // 这一步不做会一直弹窗：读借用授权时走的是 keychain 引用。
                Button("重新关联到 Magpie…") { confirmRelink = true }
                    .disabled(worker.credentialMigrationBusy)
            }
            if summary.blocked > 0 {
                HStack {
                    Text("引用异常")
                    Spacer()
                    Text("\(summary.blocked) 条").foregroundStyle(.red)
                }
            }
            if summary.rollbackable > 0 {
                HStack {
                    Text("可回滚（本次迁移产生）")
                    Spacer()
                    Text("\(summary.rollbackable) 条").foregroundStyle(.secondary)
                }
            }
            if let verify {
                HStack {
                    Text("最近核验")
                    Spacer()
                    Text(verifySummary(verify))
                        .foregroundStyle(verify.hasProblems ? .red : .secondary)
                }
            }

            Text("迁移会把**自有**授权从钥匙串复制成本机 JSON 文件（明文）。迁移前会先备份数据库与清单（备份不含令牌），原始钥匙串条目一个都不删。回滚只针对本次迁移产生的副本：之后重新登录过的授权不会被切回旧令牌；副本删不掉时记录会留着，下次回滚会重试。")
                .font(.caption)
                .foregroundStyle(.secondary)
            Text("借用授权（Magpie）**不迁移**：重新关联只把引用改成 Magpie 文件，不复制任何令牌，也就不需要明文同意。")
                .font(.caption)
                .foregroundStyle(.secondary)

            HStack(spacing: 8) {
                Button("刷新") {
                    Task { await worker.refreshCredentialMigration() }
                }
                .disabled(worker.credentialMigrationBusy)

                Button("迁移全部…") { confirmMigrate = true }
                    .disabled(worker.credentialMigrationBusy || summary.migratable == 0)

                Button("核验") {
                    Task { await worker.verifyCredentials() }
                }
                .disabled(worker.credentialMigrationBusy)

                Button("回滚…") { confirmRollback = true }
                    .disabled(worker.credentialMigrationBusy || summary.rollbackable == 0)

                if worker.credentialMigrationBusy {
                    ProgressView().controlSize(.small)
                }
            }

            if let notice = worker.credentialMigrationNotice {
                Text(notice)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }

            if let items = state?.items, !items.isEmpty {
                DisclosureGroup("逐条状态（\(items.count)）") {
                    ForEach(items) { item in
                        HStack(alignment: .firstTextBaseline) {
                            Text(item.nickname.isEmpty ? item.accountId : item.nickname)
                            Text(item.source).font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Text(statusText(item))
                                .font(.caption)
                                .foregroundStyle(statusColor(item))
                        }
                    }
                }
            }
        }
        .task { await worker.refreshCredentialMigration() }
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
        .confirmationDialog(
            "把引用改回钥匙串？",
            isPresented: $confirmRollback
        ) {
            Button("回滚到钥匙串") {
                Task { await worker.rollbackCredentialMigration() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("只回滚本次迁移产生、之后没有被重新授权的副本。副本删除前会核对引用；凭据重新从钥匙串读取（可能触发系统授权框）。")
        }
        .confirmationDialog(
            "把借用授权的引用改成 Magpie 文件？",
            isPresented: $confirmRelink
        ) {
            Button("重新关联（不复制令牌）") {
                Task { await worker.relinkBorrowedCredentials() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("只改引用，不读取、不复制、不删除任何令牌；旧钥匙串条目原样保留。需要 Magpie 已登录且该账号能唯一匹配。")
        }
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
        if item.blocked == "needs_relink" { return .orange }
        if item.blocked != nil { return .secondary }
        return item.migratable ? .orange : .secondary
    }
}
