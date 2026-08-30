import Foundation
import WidgetKit

struct WidgetDisplaySignature: Equatable {
    let codexPrimaryRemaining: Double?
    let codexFiveHourRemaining: Double?
    let codexReset: String?
    let workbuddyPoints: Double?
    let deepseekBalance: String?
    let deepseekCurrency: String?
    let deepseekIsOnline: Bool

    init(from response: StatusResponse) {
        self.codexPrimaryRemaining = response.codex?.weekly?.remaining ?? response.codex?.five_hour?.remaining
        self.codexFiveHourRemaining = response.codex?.five_hour?.remaining
        self.codexReset = (response.codex?.weekly?.reset ?? response.codex?.five_hour?.reset)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        self.workbuddyPoints = response.workbuddy?.points
        let chosenBalance = response.deepseek?.balances?.first(where: { $0.currency == "CNY" })
            ?? response.deepseek?.balances?.first
        let total = chosenBalance?.total_balance?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let curr = chosenBalance?.currency?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let displayTotal = Double(total).map { String(format: "%.2f", $0) } ?? total
        self.deepseekBalance = total.isEmpty ? nil : displayTotal
        self.deepseekCurrency = total.isEmpty ? nil : (curr.isEmpty ? "CNY" : curr)
        self.deepseekIsOnline = response.deepseek?.status?.trimmingCharacters(in: .whitespacesAndNewlines) == "Online"
    }

    init(
        codexPrimaryRemaining: Double? = nil,
        codexFiveHourRemaining: Double? = nil,
        codexReset: String? = nil,
        workbuddyPoints: Double? = nil,
        deepseekBalance: String? = nil,
        deepseekCurrency: String? = nil,
        deepseekIsOnline: Bool = false
    ) {
        self.codexPrimaryRemaining = codexPrimaryRemaining
        self.codexFiveHourRemaining = codexFiveHourRemaining
        self.codexReset = codexReset
        self.workbuddyPoints = workbuddyPoints
        self.deepseekBalance = deepseekBalance
        self.deepseekCurrency = deepseekCurrency
        self.deepseekIsOnline = deepseekIsOnline
    }

    static func shouldReloadWidget(
        previous: WidgetDisplaySignature?,
        current: WidgetDisplaySignature,
        force: Bool
    ) -> Bool {
        if force || previous == nil {
            return true
        }
        return previous != current
    }
}

@MainActor
class APIService: ObservableObject {
    static let shared = APIService()

    @Published var status: StatusResponse?
    @Published var state: DataSourceState = .loading
    @Published var lastRefresh: Date?
    @Published var errorMessage: String?

    private let baseURL: String
    private var refreshTask: Task<Void, Never>?
    private var fetchInFlight = false
    private let session: URLSession
    private var lastWidgetSignature: WidgetDisplaySignature?

    convenience init() {
        self.init(baseURL: "http://127.0.0.1:8765")
    }

    init(baseURL: String, session: URLSession? = nil) {
        self.baseURL = baseURL
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 5
        config.timeoutIntervalForResource = 10
        self.session = session ?? URLSession(configuration: config)
    }

    func fetchStatus(force: Bool = false) async {
        guard !fetchInFlight else { return }
        fetchInFlight = true
        defer { fetchInFlight = false }

        let path = force ? "/api/refresh" : "/api/status"
        let method = force ? "POST" : "GET"

        guard let url = URL(string: "\(baseURL)\(path)") else {
            state = .error("Invalid URL")
            return
        }

        var request = URLRequest(url: url)
        request.httpMethod = method
        request.cachePolicy = .reloadIgnoringLocalCacheData

        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                state = .error("Server error")
                return
            }
            let decoder = JSONDecoder()
            let decoded = try decoder.decode(StatusResponse.self, from: data)
            status = decoded
            lastRefresh = Date()
            state = .ready
            errorMessage = nil

            let newSignature = WidgetDisplaySignature(from: decoded)
            if WidgetDisplaySignature.shouldReloadWidget(
                previous: lastWidgetSignature,
                current: newSignature,
                force: force
            ) {
                lastWidgetSignature = newSignature
                WidgetCenter.shared.reloadAllTimelines()
            }
        } catch let decodingError as DecodingError {
            let detail = decodingError.failureReason ?? decodingError.localizedDescription
            state = .error(detail)
            errorMessage = detail
        } catch let urlError as URLError {
            if urlError.code == .cannotConnectToHost || urlError.code == .timedOut {
                state = .unavailable
                errorMessage = "Cannot connect to AICC server"
            } else {
                state = .error(urlError.localizedDescription)
                errorMessage = urlError.localizedDescription
            }
        } catch {
            state = .error(error.localizedDescription)
            errorMessage = error.localizedDescription
        }
    }

    /// Reconnect WorkBuddy through its fixed local endpoint, then reload the
    /// cached status used by the dashboard.
    func reconnectWorkBuddy() async {
        guard let url = URL(string: "\(baseURL)/api/workbuddy/reconnect") else { return }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        do {
            let (_, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else { return }
        } catch {
            return
        }
        await fetchStatus()
    }

    func startAutoRefresh(interval: TimeInterval) {
        refreshTask?.cancel()
        let interval = max(60, min(600, interval))
        refreshTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
                if Task.isCancelled { break }
                await fetchStatus()
            }
        }
    }

    func stopAutoRefresh() {
        refreshTask?.cancel()
        refreshTask = nil
    }

    var allServicesOk: Bool {
        guard let s = status else { return false }
        let codexOk = s.codex?.five_hour?.remaining != nil || s.codex?.weekly?.remaining != nil
        let wbOk = s.workbuddy?.points != nil
        let systemOk = s.system?.status == "Online"
        // DeepSeek might not be configured — treat that as a healthy optional source.
        let dsStatus = s.deepseek?.status
        let dsOk = dsStatus == nil || dsStatus == "Online" || dsStatus == "Not configured"
        return codexOk && wbOk && dsOk && systemOk
    }
}
