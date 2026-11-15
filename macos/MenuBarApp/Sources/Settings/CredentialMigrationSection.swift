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

    private var state: CredentialMigrationState? { worker.credentialMigration }
    private var summary: CredentialMigrationSummary { state?.summary ?? .empty }

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
            if summary.magpie > 0 {
                HStack {
                    Text("由 Magpie 管理（不迁移）")
                    Spacer()
                    Text("\(summary.magpie) 条").foregroundStyle(.secondary)
                }
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

            Text("迁移会把钥匙串里的旧令牌**复制**成本机 JSON 文件（明文）。迁移前会先备份数据库与清单（备份不含令牌），原始钥匙串条目一个都不删。回滚只针对**本次迁移产生**的副本：之后重新登录过的授权不会被切回旧令牌。")
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
    }

    private func statusText(_ item: CredentialMigrationItem) -> String {
        if let blocked = item.blockedText { return blocked }
        return item.storageText
    }

    private func statusColor(_ item: CredentialMigrationItem) -> Color {
        if item.blocked != nil { return .secondary }
        return item.migratable ? .orange : .secondary
    }
}
