import Foundation

// MARK: - API Response
struct StatusResponse: Codable {
    let codex: CodexData?
    let workbuddy: WorkBuddyData?
    let deepseek: DeepSeekData?
    let system: SystemData?
    let collection: CollectionMeta?
    let updated_at: String?
}

// MARK: - Codex
struct CodexData: Codable {
    let five_hour: RateWindow?
    let weekly: RateWindow?
    let source: String?
    let state: String?
    let stale: Bool?
}

struct RateWindow: Codable {
    let remaining: Double?
    let reset: String?
    let label: String?
}

// MARK: - WorkBuddy
struct WorkBuddyData: Codable {
    let points: Double?
    let used_points: Double?
    let total_points: Double?
    let reset_text: String?
    let balance_state: String?
    let auto_used_credits: Double?
    let usage_records: Int?
}

// MARK: - DeepSeek
struct DeepSeekData: Codable {
    let status: String?
    let balances: [DeepSeekBalance]?
    let usage: [DeepSeekUsage]?
    let source: String?
}

struct DeepSeekBalance: Codable {
    let currency: String?
    let total_balance: String?
    let granted_balance: String?
    let topped_up_balance: String?
}

struct DeepSeekUsage: Codable {
    let currency: String?
    let used_today: String?
}

// MARK: - System
struct SystemData: Codable {
    let label: String?
    let status: String?
    let cpu: String?
    let platform: String?
    let ram: String?
    let gpu: String?
}

// MARK: - Collection Metadata
struct CollectionMeta: Codable {
    let codex: String?
    let workbuddy: String?
    let deepseek: String?
    let system: String?
}

// MARK: - OpenCodex Status
enum OCXStatus: Equatable {
    case unknown
    case detecting
    case notFound
    case stopped
    case starting
    case running
    case stopping
    case error(String)

    var label: String {
        switch self {
        case .unknown: return "Checking..."
        case .detecting: return "Detecting..."
        case .notFound: return "Not Found"
        case .stopped: return "Stopped"
        case .starting: return "Starting..."
        case .running: return "Running"
        case .stopping: return "Stopping..."
        case .error(let msg): return msg
        }
    }

    var isRunning: Bool {
        if case .running = self { return true }
        return false
    }
}

// MARK: - Data Source State
enum DataSourceState {
    case loading
    case ready
    case stale
    case unavailable
    case error(String)
}
