import Foundation

@MainActor
class APIService: ObservableObject {
    static let shared = APIService()

    @Published var status: StatusResponse?
    @Published var health: HealthResponse?
    @Published var state: DataSourceState = .loading
    @Published var lastRefresh: Date?
    @Published var errorMessage: String?

    private let baseURL: String
    private var refreshTask: Task<Void, Never>?
    private var healthTask: Task<Void, Never>?
    private var fetchInFlight = false
    private var healthInFlight = false
    private let session: URLSession

    init() {
        let port = ProcessInfo.processInfo.environment["EINK_PORT"] ?? "8765"
        baseURL = "http://127.0.0.1:\(port)"
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 5
        config.timeoutIntervalForResource = 10
        session = URLSession(configuration: config)
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

    func fetchHealth() async {
        guard !healthInFlight else { return }
        healthInFlight = true
        defer { healthInFlight = false }

        guard let url = URL(string: "\(baseURL)/api/health") else { return }
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 5

        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse,
                  (200..<500).contains(http.statusCode) else { return }
            health = try JSONDecoder().decode(HealthResponse.self, from: data)
        } catch {
            // Health is diagnostic state. Do not overwrite a usable cached status
            // just because this low-frequency probe was unavailable.
        }
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
        healthTask?.cancel()
        healthTask = nil
    }

    func startHealthRefresh(interval: TimeInterval = 60) {
        healthTask?.cancel()
        let interval = max(60, min(300, interval))
        healthTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
                if Task.isCancelled { break }
                await fetchHealth()
            }
        }
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
