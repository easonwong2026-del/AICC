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
    let available: Bool?
}

struct RateWindow: Codable {
    let remaining: Double?
    let reset: String?
    let label: String?
    let duration_minutes: Int?
}

// MARK: - OpenCodex Provider Quota

struct OCXProviderQuotaResponse: Decodable, Equatable {
    let generatedAt: Double?
    let reports: [OCXProviderQuotaReport]

    init(jsonData: Data) throws {
        self = try JSONDecoder().decode(Self.self, from: jsonData)
    }

    private enum CodingKeys: String, CodingKey {
        case generatedAt
        case reports
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        generatedAt = (try? container.decode(OCXLossyDouble.self, forKey: .generatedAt))?.value
        reports = (try? container.decode([OCXProviderQuotaReport].self, forKey: .reports)) ?? []
    }
}

struct OCXProviderQuotaReport: Decodable, Equatable {
    let provider: String?
    let label: String?
    let source: String?
    let quota: OCXProviderQuota?

    private enum CodingKeys: String, CodingKey {
        case provider
        case label
        case source
        case quota
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        provider = try? container.decode(String.self, forKey: .provider)
        label = try? container.decode(String.self, forKey: .label)
        source = try? container.decode(String.self, forKey: .source)
        quota = try? container.decode(OCXProviderQuota.self, forKey: .quota)
    }
}

struct OCXProviderQuota: Decodable, Equatable {
    let customWindows: [OCXProviderQuotaWindow]?
    let updatedAt: Double?

    private enum CodingKeys: String, CodingKey {
        case customWindows
        case updatedAt
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        customWindows = try? container.decode([OCXProviderQuotaWindow].self, forKey: .customWindows)
        updatedAt = (try? container.decode(OCXLossyDouble.self, forKey: .updatedAt))?.value
    }
}

struct OCXProviderQuotaWindow: Decodable, Equatable {
    let label: String?
    let percent: Double?
    let resetAt: Date?

    var remainingPercent: Double? {
        guard let percent, percent.isFinite else { return nil }
        return min(max(100 - percent, 0), 100)
    }

    private enum CodingKeys: String, CodingKey {
        case label
        case percent
        case resetAt
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        label = try? container.decode(String.self, forKey: .label)
        percent = (try? container.decode(OCXLossyDouble.self, forKey: .percent))?.value
        resetAt = (try? container.decode(OCXLossyDate.self, forKey: .resetAt))?.value
    }
}

struct GoogleQuota: Equatable {
    let gem: OCXProviderQuotaWindow?
    let cla: OCXProviderQuotaWindow?
    let updatedAt: Date?
}

enum OCXGoogleQuotaState: Equatable {
    case unknown
    case loading
    case live
    case stale
    case noData
    case stopped
    case notInstalled
    case unavailable
}

enum OCXProviderQuotaParser {
    static func googleQuota(from response: OCXProviderQuotaResponse) -> GoogleQuota? {
        guard
            let report = response.reports.first(where: { $0.provider == "google-antigravity" }),
            let quota = report.quota
        else {
            return nil
        }

        let windows = quota.customWindows ?? []
        return GoogleQuota(
            gem: windows.first(where: { $0.label == "Gem" }),
            cla: windows.first(where: { $0.label == "Cla" }),
            updatedAt: quota.updatedAt.flatMap { Date(timeIntervalSince1970: $0) }
        )
    }
}

// MARK: - WorkBuddy
struct WorkBuddyData: Codable {
    let points: Double?
    let used_points: Double?
    let total_points: Double?
    let reset_text: String?
    let balance_state: String?
    let balance_stale: Bool?
    let balance_updated_at: String?
    let balance_updated_epoch: Double?
    let balance_age_seconds: Int?
    let balance_error_code: String?
    let balance_error: String?
    let auto_used_credits: Double?
    let usage_records: Int?
    let usage_source: String?
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
struct CollectorStatus: Codable {
    let state: String?
    let last_success: String?
    let age_seconds: Int?
    let error: String?
    let refresh_seconds: Int?
}

struct CollectionMeta: Codable {
    let codex: CollectorStatus?
    let workbuddy: CollectorStatus?
    let deepseek: CollectorStatus?
    let system: CollectorStatus?
}

// MARK: - OpenCodex Status
enum OCXStatus: Equatable {
    case unknown
    case checking
    case notInstalled
    case stopped
    case starting
    case running
    case stopping
    case unhealthy

    var label: String {
        switch self {
        case .unknown: return "Unknown"
        case .checking: return "Checking..."
        case .notInstalled: return "Not Installed"
        case .stopped: return "Stopped"
        case .starting: return "Starting..."
        case .running: return "Running"
        case .stopping: return "Stopping..."
        case .unhealthy: return "unhealthy"
        }
    }

    var isRunning: Bool {
        self == .running
    }

    /// The switch remains on for an unhealthy but active service so the user
    /// can turn it off and issue a manual stop command.
    var isToggleOn: Bool {
        switch self {
        case .running, .starting, .unhealthy:
            return true
        case .unknown, .checking, .notInstalled, .stopped, .stopping:
            return false
        }
    }

    var isBusy: Bool {
        switch self {
        case .checking, .starting, .stopping:
            return true
        case .unknown, .notInstalled, .stopped, .running, .unhealthy:
            return false
        }
    }
}

enum OCXUpdateState: Equatable {
    case idle
    case checking
    case upToDate
    case available(String)
    case updating
    case updated(from: String, to: String, restartRequired: Bool)
    case failed(String)

    var isBusy: Bool {
        switch self {
        case .checking, .updating:
            return true
        case .idle, .upToDate, .available, .updated, .failed:
            return false
        }
    }

    static func checkResult(current: String, latest: String?) -> OCXUpdateState {
        guard
            let latest,
            let currentVersion = SemanticVersion(current),
            let latestVersion = SemanticVersion(latest)
        else {
            return .failed("Unable to check for updates")
        }
        let normalizedLatest = latestVersion.description
        return latestVersion > currentVersion
            ? .available(normalizedLatest)
            : .upToDate
    }

    static func completion(
        from oldVersion: String,
        to newVersion: String?,
        restartRequired: Bool
    ) -> OCXUpdateState {
        guard let newVersion else {
            return .failed("OpenCodex update completed, but version could not be verified.")
        }
        guard
            let old = SemanticVersion(oldVersion),
            let new = SemanticVersion(newVersion)
        else {
            return .failed("OpenCodex update completed, but version could not be verified.")
        }
        guard old != new else {
            return .failed("OpenCodex update completed, but version did not change.")
        }
        return .updated(
            from: old.description,
            to: new.description,
            restartRequired: restartRequired
        )
    }
}

enum OCXOperationPolicy {
    static let panelPollIntervalNanoseconds: UInt64 = 9_000_000_000
    static let statusTimeout: TimeInterval = 4.5
    static let providerQuotaTTL: TimeInterval = 5 * 60
    static let providerQuotaTimeout: TimeInterval = 8
    static let operationTimeout: TimeInterval = 12
    static let updateCheckTimeout: TimeInterval = 12
    static let updateTimeout: TimeInterval = 180
    static let confirmationDelays: [UInt64] = [
        300_000_000,
        700_000_000,
        1_500_000_000,
        3_000_000_000
    ]

    static func shouldApplyStatus(
        requestGeneration: Int,
        currentGeneration: Int,
        operationActive: Bool
    ) -> Bool {
        requestGeneration == currentGeneration && !operationActive
    }

    static func shouldContinuePanel(isVisible: Bool, taskIsCancelled: Bool) -> Bool {
        isVisible && !taskIsCancelled
    }

    static func shouldRefreshProviderQuota(
        force: Bool,
        lastAttempt: Date?,
        now: Date
    ) -> Bool {
        guard !force else { return true }
        guard let lastAttempt else { return true }
        return now.timeIntervalSince(lastAttempt) >= providerQuotaTTL
    }

    static func reachedTarget(_ observed: OCXStatus, target: OCXStatus) -> Bool {
        observed == target
    }
}

enum OCXVersionParser {
    static func parse(_ output: String) -> String? {
        guard let firstLine = output.split(whereSeparator: { $0 == "\n" || $0 == "\r" }).first else {
            return nil
        }
        let value = String(firstLine).trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }

    static func semanticVersion(from output: String) -> SemanticVersion? {
        guard let line = parse(output) else { return nil }
        return line
            .split(whereSeparator: { $0 == " " || $0 == "\t" })
            .reversed()
            .compactMap { SemanticVersion(String($0)) }
            .first
    }
}

enum OCXSnapshotError: Error, Equatable {
    case invalidJSON
    case missingRequiredField(String)
}

/// The stable, provider-facing result of `ocx status --json`.
///
/// The decoder intentionally accepts optional fields and ignores additions to
/// the CLI schema. `proxy.running` is the only required field; a running proxy
/// without a boolean health result is treated as unhealthy by `resolvedStatus`.
struct OCXSnapshot: Equatable {
    let running: Bool
    let pid: Int?
    let port: Int?
    let healthOK: Bool?
    let healthURL: URL?
    let healthMessage: String?
    let dashboardURL: URL?
    let version: String?

    var resolvedStatus: OCXStatus {
        guard running else { return .stopped }
        return healthOK == true ? .running : .unhealthy
    }

    var hasValidDashboardURL: Bool {
        dashboardURL != nil
    }

    init(jsonData: Data) throws {
        let document: OCXDocument
        do {
            document = try JSONDecoder().decode(OCXDocument.self, from: jsonData)
        } catch {
            throw OCXSnapshotError.invalidJSON
        }

        guard let proxy = document.proxy else {
            throw OCXSnapshotError.missingRequiredField("proxy")
        }
        guard let running = proxy.running else {
            throw OCXSnapshotError.missingRequiredField("proxy.running")
        }

        self.running = running
        self.pid = proxy.pid
        self.port = document.listen?.port
        self.healthOK = proxy.health?.ok
        self.healthURL = Self.validHTTPURL(proxy.health?.url)
        self.healthMessage = proxy.health?.message
        self.dashboardURL = Self.validHTTPURL(document.dashboard?.url)
        self.version = document.codexRuntime?.version
    }

    static func validHTTPURL(_ value: String?) -> URL? {
        guard
            let value,
            let url = URL(string: value),
            let scheme = url.scheme?.lowercased(),
            (scheme == "http" || scheme == "https"),
            let host = url.host,
            !host.isEmpty
        else {
            return nil
        }
        return url
    }
}

private struct OCXDocument: Decodable {
    let proxy: OCXProxyDocument?
    let dashboard: OCXDashboardDocument?
    let listen: OCXListenDocument?
    let codexRuntime: OCXRuntimeDocument?

    private enum CodingKeys: String, CodingKey {
        case proxy
        case dashboard
        case listen
        case codexRuntime
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        proxy = try? container.decodeIfPresent(OCXProxyDocument.self, forKey: .proxy)
        dashboard = try? container.decodeIfPresent(OCXDashboardDocument.self, forKey: .dashboard)
        listen = try? container.decodeIfPresent(OCXListenDocument.self, forKey: .listen)
        codexRuntime = try? container.decodeIfPresent(OCXRuntimeDocument.self, forKey: .codexRuntime)
    }
}

private struct OCXProxyDocument: Decodable {
    let running: Bool?
    let pid: Int?
    let health: OCXHealthDocument?

    private enum CodingKeys: String, CodingKey {
        case running
        case pid
        case health
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        running = try? container.decodeIfPresent(Bool.self, forKey: .running)
        pid = try? container.decodeIfPresent(LossyInt.self, forKey: .pid)?.value
        health = try? container.decodeIfPresent(OCXHealthDocument.self, forKey: .health)
    }
}

private struct OCXHealthDocument: Decodable {
    let ok: Bool?
    let url: String?
    let message: String?

    private enum CodingKeys: String, CodingKey {
        case ok
        case url
        case message
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        ok = try? container.decodeIfPresent(Bool.self, forKey: .ok)
        url = try? container.decodeIfPresent(String.self, forKey: .url)
        message = try? container.decodeIfPresent(String.self, forKey: .message)
    }
}

private struct OCXDashboardDocument: Decodable {
    let url: String?

    private enum CodingKeys: String, CodingKey {
        case url
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        url = try? container.decodeIfPresent(String.self, forKey: .url)
    }
}

private struct OCXListenDocument: Decodable {
    let port: Int?

    private enum CodingKeys: String, CodingKey {
        case port
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        port = try? container.decodeIfPresent(LossyInt.self, forKey: .port)?.value
    }
}

private struct OCXRuntimeDocument: Decodable {
    let version: String?

    private enum CodingKeys: String, CodingKey {
        case version
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        version = try? container.decodeIfPresent(String.self, forKey: .version)
    }
}

private struct OCXLossyDouble: Decodable {
    let value: Double?

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            value = nil
        } else if let number = try? container.decode(Double.self), number.isFinite {
            value = number
        } else if let string = try? container.decode(String.self) {
            value = Double(string.trimmingCharacters(in: .whitespacesAndNewlines))
        } else {
            value = nil
        }
    }
}

private struct OCXLossyDate: Decodable {
    let value: Date?

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let number = try? container.decode(Double.self), number.isFinite {
            value = Date(timeIntervalSince1970: number)
        } else if let string = try? container.decode(String.self) {
            let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
            if let seconds = Double(trimmed), seconds.isFinite {
                value = Date(timeIntervalSince1970: seconds)
            } else {
                value = ISO8601DateFormatter().date(from: trimmed)
            }
        } else {
            value = nil
        }
    }
}

private struct LossyInt: Decodable {
    let value: Int?

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            value = nil
        } else if let number = try? container.decode(Int.self) {
            value = number
        } else if let string = try? container.decode(String.self) {
            value = Int(string.trimmingCharacters(in: .whitespacesAndNewlines))
        } else {
            value = nil
        }
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
// MARK: - OpenCodex Command Builder

/// A safe, inspectable invocation for OpenCodex lifecycle commands that must
/// execute through a login shell so the user's .zprofile, Homebrew, npm-global
/// paths, and CODEX_CLI_PATH are available.
///
/// The detected `ocx` binary path is passed via environment variable
/// `AICC_OCX_PATH`, and the shell command references only that variable.
/// No path string is concatenated into the shell argument, so spaces,
/// single quotes, and other special characters in the path are safe.
///
/// Only lifecycle commands (`ensure`, `stop`) use the shell invocation.
/// Read-only operations execute the detected binary directly.
struct OCXCommandInvocation: Equatable {
    let executable: String
    let arguments: [String]
    let environmentOverrides: [String: String]
}

enum OCXCommandBuilder {
    private static let envKey = "AICC_OCX_PATH"
    private static let packageName = "@bitkyc08/opencodex"
    private static let allowedCommands: Set<String> = ["ensure", "stop"]

    /// Build a login-shell invocation for a lifecycle command.
    ///
    /// - Parameters:
    ///   - command: `"ensure"` or `"stop"`.
    ///   - ocxPath: The filesystem path to the discovered `ocx` binary.
    /// - Returns: An `OCXCommandInvocation` ready to pass to `ProcessRunner`.
    /// - Precondition: `command` must be one of the allowed lifecycle commands.
    ///   A runtime assertion guards against accidental misuse.
    static func lifecycle(command: String, ocxPath: String) -> OCXCommandInvocation {
        assert(allowedCommands.contains(command),
               "OCXCommandBuilder only accepts lifecycle commands: \(allowedCommands)")
        return OCXCommandInvocation(
            executable: "/bin/zsh",
            arguments: ["-lc", "exec \"$\(envKey)\" \(command)"],
            environmentOverrides: [envKey: ocxPath]
        )
    }

    static func updateCheck() -> OCXCommandInvocation {
        OCXCommandInvocation(
            executable: "/usr/bin/env",
            arguments: ["npm", "view", "\(packageName)@latest", "version"],
            environmentOverrides: [:]
        )
    }

    static func update(ocxPath: String) -> OCXCommandInvocation {
        OCXCommandInvocation(
            executable: ocxPath,
            arguments: ["update"],
            environmentOverrides: [:]
        )
    }

    static func providerQuota(ocxPath: String, force: Bool) -> OCXCommandInvocation {
        OCXCommandInvocation(
            executable: ocxPath,
            arguments: ["provider", "quota"] + (force ? ["--refresh"] : []) + ["--json"],
            environmentOverrides: [:]
        )
    }
}
