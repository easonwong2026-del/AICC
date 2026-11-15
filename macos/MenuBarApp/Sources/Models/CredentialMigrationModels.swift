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

    static let empty = CredentialMigrationSummary(
        migratable: 0,
        alreadyJSON: 0,
        magpie: 0,
        blocked: 0,
        rollbackable: 0,
        total: 0
    )

    /// 一条都不需要迁（含"全是 JSON / 只有 Magpie"）。
    var needsAttention: Bool { migratable > 0 || blocked > 0 }
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

    var id: String { "\(domain)/\(accountId)/\(source)" }

    var blockedText: String? {
        switch blocked {
        case "already_json": return "已经是本地 JSON"
        case "magpie_managed": return "由 Magpie 管理，不迁移"
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

/// 一次盘点结果。
struct CredentialMigrationState: Equatable {
    var summary: CredentialMigrationSummary
    var items: [CredentialMigrationItem]

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
                blocked: raw["blocked"] as? String
            )
        }
        return CredentialMigrationState(
            summary: CredentialMigrationSummary(
                migratable: summary["migratable"] as? Int ?? 0,
                alreadyJSON: summary["alreadyJson"] as? Int ?? 0,
                magpie: summary["magpie"] as? Int ?? 0,
                blocked: summary["blocked"] as? Int ?? 0,
                rollbackable: summary["rollbackable"] as? Int ?? 0,
                total: summary["total"] as? Int ?? 0
            ),
            items: items
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

    /// 有需要人处理的问题（异常条目 / 孤儿文件 / 坏文件）。
    var hasProblems: Bool { broken > 0 || orphanCount > 0 || brokenFiles > 0 }

    static func parse(_ json: [String: Any]) -> CredentialVerifyReport? {
        guard json["ok"] != nil else { return nil }
        return CredentialVerifyReport(
            ok: json["ok"] as? Int ?? 0,
            broken: json["broken"] as? Int ?? 0,
            pendingKeychain: json["pendingKeychain"] as? Int ?? 0,
            orphanCount: (json["orphans"] as? [Any])?.count ?? 0,
            brokenFiles: json["brokenFiles"] as? Int ?? 0
        )
    }
}
