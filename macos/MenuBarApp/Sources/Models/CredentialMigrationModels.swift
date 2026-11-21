import Foundation

/// CRED-4：旧 Keychain 凭据迁移的**只读视图**。
///
/// 这里的每个字段都来自助手的 `credentials.migration.*` RPC，界面不自己推断：
/// 迁移是否可行、哪一条被阻塞、核验有没有发现孤儿文件，全部以后端为准。
/// 结构里**不含任何令牌内容**（引用是文件 ID / 键名，指纹是摘要）。

/// 盘点摘要。
struct CredentialMigrationSummary: Equatable {
    var migratable: Int
    var alreadyJSON: Int
    var magpie: Int
    var blocked: Int
    /// 有迁移记录、且当前行仍是那次迁移产物的条数 —— 只有这些能回滚。
    var rollbackable: Int
    var total: Int
    /// CRED-5：借用授权仍指着钥匙串（`keychain:` 引用）。
    ///
    /// 这类行**不迁移**（会把 Magpie 的 access 复制成明文文件），只做一次
    /// 显式的重新关联：改引用、一个令牌都不搬。
    var needsRelink: Int = 0

    static let empty = CredentialMigrationSummary(
        migratable: 0,
        alreadyJSON: 0,
        magpie: 0,
        blocked: 0,
        rollbackable: 0,
        total: 0,
        needsRelink: 0
    )

    /// 一条都不需要迁（含"全是 JSON / 只有 Magpie"）。
    var needsAttention: Bool { migratable > 0 || blocked > 0 || needsRelink > 0 }
}

/// 盘点里的一条授权。
struct CredentialMigrationItem: Equatable, Identifiable {
    var domain: String
    var platform: String
    var accountId: String
    var nickname: String
    var source: String
    var storage: String
    var reference: String?
    var migratable: Bool
    var rollbackable: Bool
    var blocked: String?
    /// CRED-5：后端给出的下一步动作。目前只有 `relink`。
    /// 界面按这个字段决定按钮，**不**去匹配 `blocked` 的字符串。
    var action: String? = nil

    var id: String { "\(domain)/\(accountId)/\(source)" }

    var blockedText: String? {
        switch blocked {
        case "already_json": return "已经是本地 JSON"
        case "magpie_managed": return "由 Magpie 管理，不迁移"
        case "needs_relink": return "借用授权仍指向钥匙串，需重新关联"
        case "reference_unusable": return "引用不可用"
        case nil: return nil
        default: return blocked
        }
    }

    var storageText: String {
        switch storage {
        case "keychain": return "钥匙串"
        case "localjson": return "本地 JSON"
        case "magpie": return "Magpie"
        default: return storage
        }
    }
}

/// 旧键搬运（G4 时代：旧钥匙串键 → 新键）的状态。
///
/// 它是**另一套**状态机，和 `credential_migrations` 无关：`failed` 表示用户
/// 拒绝过钥匙串授权，或者写入失败，**不会**被启动路径自动重试。
struct CredentialKeyMigration: Equatable {
    var failed: Int
    var pending: Int

    static let empty = CredentialKeyMigration(failed: 0, pending: 0)

    /// 有失败行：给一个显式的"重试"按钮，写清楚会弹授权框。
    var needsAttention: Bool { failed > 0 }
}

/// 一次盘点结果。
struct CredentialMigrationState: Equatable {
    var summary: CredentialMigrationSummary
    var items: [CredentialMigrationItem]
    /// 库里有迁移记录的条数（含后来被重新授权、记录已不再描述当前行的）。
    var migratedRecords: Int = 0
    /// 旧键搬运的失败 / 待处理条数。
    var keyMigration: CredentialKeyMigration = .empty

    static let empty = CredentialMigrationState(summary: .empty, items: [])

    static func parse(_ json: [String: Any]) -> CredentialMigrationState? {
        guard let summary = json["summary"] as? [String: Any] else { return nil }
        let items = (json["items"] as? [[String: Any]] ?? []).map { raw in
            CredentialMigrationItem(
                domain: raw["domain"] as? String ?? "",
                platform: raw["platform"] as? String ?? "",
                accountId: raw["accountId"] as? String ?? "",
                nickname: raw["nickname"] as? String ?? "",
                source: raw["source"] as? String ?? "",
                storage: raw["storage"] as? String ?? "",
                reference: raw["reference"] as? String,
                migratable: raw["migratable"] as? Bool ?? false,
                rollbackable: raw["rollbackable"] as? Bool ?? false,
                blocked: raw["blocked"] as? String,
                action: raw["action"] as? String
            )
        }
        return CredentialMigrationState(
            summary: CredentialMigrationSummary(
                migratable: summary["migratable"] as? Int ?? 0,
                alreadyJSON: summary["alreadyJson"] as? Int ?? 0,
                magpie: summary["magpie"] as? Int ?? 0,
                blocked: summary["blocked"] as? Int ?? 0,
                rollbackable: summary["rollbackable"] as? Int ?? 0,
                total: summary["total"] as? Int ?? 0,
                needsRelink: summary["needsRelink"] as? Int ?? 0
            ),
            items: items,
            migratedRecords: json["migratedRecords"] as? Int ?? 0,
            keyMigration: {
                guard let raw = json["keyMigration"] as? [String: Any] else { return .empty }
                return CredentialKeyMigration(
                    failed: raw["failed"] as? Int ?? 0,
                    pending: raw["pending"] as? Int ?? 0
                )
            }()
        )
    }
}

/// 核验结果。
struct CredentialVerifyReport: Equatable {
    var ok: Int
    var broken: Int
    var pendingKeychain: Int
    var orphanCount: Int
    var brokenFiles: Int
    /// 回滚过、但副本文件没删掉的份数。**不是故障**：记录还在，下次回滚会重试。
    var pendingCleanup: Int
    /// CRED-5：借用授权还指着钥匙串（可重新关联，不属于"待迁移"）。
    var needsRelink: Int = 0

    /// 有需要人处理的问题（异常条目 / 孤儿文件 / 坏文件）。
    ///
    /// 待清理副本不算：它已经有了可重试的去处，不该让整块面板一直标红。
    var hasProblems: Bool { broken > 0 || orphanCount > 0 || brokenFiles > 0 }

    static func parse(_ json: [String: Any]) -> CredentialVerifyReport? {
        guard json["ok"] != nil else { return nil }
        return CredentialVerifyReport(
            ok: json["ok"] as? Int ?? 0,
            broken: json["broken"] as? Int ?? 0,
            pendingKeychain: json["pendingKeychain"] as? Int ?? 0,
            orphanCount: (json["orphans"] as? [Any])?.count ?? 0,
            brokenFiles: json["brokenFiles"] as? Int ?? 0,
            pendingCleanup: (json["pendingCleanup"] as? [Any])?.count ?? 0,
            needsRelink: json["needsRelink"] as? Int ?? 0
        )
    }
}

/// CRED-5：批量重新关联的失败原因聚合。
///
/// 放在纯模型里而不是 `AccountWorkerManager` 上：Manager 依赖 SwiftUI，
/// 不能进 AICCCore 测试目标；这段逻辑只处理字典，属于可测的展示规则。
enum CredentialRelinkSummary {
    /// 把 `items[]` 里 `ok == false` 的原因聚合成一句，按出现次数排序。
    static func reasonSummary(_ items: [[String: Any]]) -> String {
        var counts: [String: Int] = [:]
        for item in items where item["ok"] as? Bool == false {
            let reason = item["reason"] as? String ?? "unknown"
            counts[reason, default: 0] += 1
        }
        return counts
            .sorted { $0.value == $1.value ? $0.key < $1.key : $0.value > $1.value }
            .map { "\($0.key) ×\($0.value)" }
            .joined(separator: "、")
    }
}
