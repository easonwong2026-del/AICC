import Foundation

@MainActor
class APIService: ObservableObject {
    static let shared = APIService()

    @Published var status: StatusResponse?
    @Published var state: DataSourceState = .loading
    @Published var lastRefresh: Date?
    @Published var errorMessage: String?

    private let baseURL: String
    private var refreshTask: Task<Void, Never>?
    private let session: URLSession

    init() {
        let port = ProcessInfo.processInfo.environment["EINK_PORT"] ?? "8765"
        baseURL = "http://127.0.0.1:\(port)"
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 5
        config.timeoutIntervalForResource = 10
        session = URLSession(configuration: config)
        Task { await fetchStatus() }
    }

    func fetchStatus(force: Bool = false) async {
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
            let decoded = try JSONDecoder().decode(StatusResponse.self, from: data)
            status = decoded
            lastRefresh = Date()
            state = .ready
            errorMessage = nil
        } catch {
            if (error as NSError).code == NSURLErrorCannotConnectToHost ||
               (error as NSError).code == NSURLErrorTimedOut {
                state = .unavailable
                errorMessage = "Cannot connect to AICC server"
            } else {
                state = .error(error.localizedDescription)
                errorMessage = error.localizedDescription
            }
        }
    }

    func startAutoRefresh(interval: TimeInterval) {
        refreshTask?.cancel()
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
        let dsOk = s.deepseek?.status == "Online"
        return codexOk && wbOk && dsOk
    }
}
