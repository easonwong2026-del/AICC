import Foundation

struct WidgetStatusPayload: Decodable {
    let codex: WidgetCodexData?
    let workbuddy: WidgetWorkBuddyData?
    let deepseek: WidgetDeepSeekData?
    let system: WidgetSystemData?

    enum CodingKeys: String, CodingKey {
        case codex
        case workbuddy
        case deepseek
        case system
    }
}

struct WidgetCodexData: Decodable {
    let fiveHour: WidgetRateWindow?
    let weekly: WidgetRateWindow?

    enum CodingKeys: String, CodingKey {
        case fiveHour = "five_hour"
        case weekly
    }
}

struct WidgetRateWindow: Decodable {
    let remaining: Double?
}

struct WidgetWorkBuddyData: Decodable {
    let points: Double?
}

struct WidgetDeepSeekData: Decodable {
    let balances: [WidgetDeepSeekBalance]?
}

struct WidgetDeepSeekBalance: Decodable {
    let currency: String?
    let totalBalance: String?

    enum CodingKeys: String, CodingKey {
        case currency
        case totalBalance = "total_balance"
    }
}

struct WidgetSystemData: Decodable {
    let status: String?
}

struct WidgetDisplaySnapshot: Codable, Equatable {
    let codex: String
    let workbuddy: String
    let deepseek: String
    let system: String
    let fetchedAt: Date
    let stale: Bool

    static let placeholder = WidgetDisplaySnapshot(
        codex: "—",
        workbuddy: "—",
        deepseek: "—",
        system: "—",
        fetchedAt: .now,
        stale: true
    )

    init(
        codex: String,
        workbuddy: String,
        deepseek: String,
        system: String,
        fetchedAt: Date,
        stale: Bool
    ) {
        self.codex = codex
        self.workbuddy = workbuddy
        self.deepseek = deepseek
        self.system = system
        self.fetchedAt = fetchedAt
        self.stale = stale
    }

    init(payload: WidgetStatusPayload, fetchedAt: Date) {
        self.init(
            codex: Self.formatCodex(payload.codex),
            workbuddy: Self.formatWorkBuddy(payload.workbuddy?.points),
            deepseek: Self.formatDeepSeek(payload.deepseek),
            system: Self.formatSystem(payload.system?.status),
            fetchedAt: fetchedAt,
            stale: false
        )
    }

    var staleCopy: WidgetDisplaySnapshot {
        WidgetDisplaySnapshot(
            codex: codex,
            workbuddy: workbuddy,
            deepseek: deepseek,
            system: system,
            fetchedAt: fetchedAt,
            stale: true
        )
    }

    private static func formatCodex(_ data: WidgetCodexData?) -> String {
        guard
            let remaining = data?.weekly?.remaining ?? data?.fiveHour?.remaining,
            remaining.isFinite
        else { return "—" }
        return String(format: "%.0f%%", remaining)
    }

    private static func formatWorkBuddy(_ points: Double?) -> String {
        guard let points, points.isFinite else { return "—" }
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = true
        formatter.maximumFractionDigits = 2
        formatter.minimumFractionDigits = 0
        return formatter.string(from: NSNumber(value: points)) ?? "—"
    }

    private static func formatDeepSeek(_ data: WidgetDeepSeekData?) -> String {
        guard
            let balance = data?.balances?.first(where: { $0.currency == "CNY" }) ?? data?.balances?.first,
            let total = balance.totalBalance?.trimmingCharacters(in: .whitespacesAndNewlines),
            !total.isEmpty
        else { return "—" }

        let currency = balance.currency?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return currency.isEmpty ? total : "\(total) \(currency)"
    }

    private static func formatSystem(_ status: String?) -> String {
        guard let status = status?.trimmingCharacters(in: .whitespacesAndNewlines), !status.isEmpty else {
            return "—"
        }
        return status
    }
}

enum WidgetStatusStore {
    private static let key = "aicc.widget.last-display-snapshot"

    static func load() -> WidgetDisplaySnapshot? {
        guard
            let data = UserDefaults.standard.data(forKey: key),
            let snapshot = try? JSONDecoder().decode(WidgetDisplaySnapshot.self, from: data)
        else { return nil }
        return snapshot
    }

    static func save(_ snapshot: WidgetDisplaySnapshot) {
        guard !snapshot.stale, let data = try? JSONEncoder().encode(snapshot) else { return }
        UserDefaults.standard.set(data, forKey: key)
    }

    static func cachedOrPlaceholder() -> WidgetDisplaySnapshot {
        load()?.staleCopy ?? .placeholder
    }

    static func remove() {
        UserDefaults.standard.removeObject(forKey: key)
    }
}

enum WidgetStatusLoader {
    private static let endpoint = URL(string: "http://127.0.0.1:8765/api/status")!

    static func snapshot() async -> WidgetDisplaySnapshot {
        if let fresh = await load() {
            return fresh
        }
        return WidgetStatusStore.cachedOrPlaceholder()
    }

    static func load() async -> WidgetDisplaySnapshot? {
        var request = URLRequest(url: endpoint)
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 3

        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard
                let http = response as? HTTPURLResponse,
                (200..<300).contains(http.statusCode)
            else { return nil }

            let payload = try JSONDecoder().decode(WidgetStatusPayload.self, from: data)
            let snapshot = WidgetDisplaySnapshot(payload: payload, fetchedAt: .now)
            WidgetStatusStore.save(snapshot)
            return snapshot
        } catch {
            return nil
        }
    }
}
