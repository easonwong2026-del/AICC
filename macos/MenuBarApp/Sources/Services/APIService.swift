import Foundation

@MainActor
class APIService: ObservableObject {
    static let shared = APIService()

    @Published var status: StatusResponse?
    @Published var providers: ProvidersResponse?
    /// Last provider-request failure. Kept separate from `errorMessage` so a
    /// transient `/api/providers` outage never flips the overall dashboard
    /// connection state.
    @Published var providerErrorMessage: String?
    /// When the current provider snapshot was last fetched successfully.
    @Published var providerLastSuccess: Date?
    @Published var state: DataSourceState = .loading
    @Published var lastRefresh: Date?
    @Published var errorMessage: String?

    private let baseURL: String
    private var refreshTask: Task<Void, Never>?
    private var fetchInFlight = false
    private let session: URLSession

    convenience init() {
        let port = ProcessInfo.processInfo.environment["EINK_PORT"] ?? "8765"
        self.init(baseURL: "http://127.0.0.1:\(port)")
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
            await fetchProviders()
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

    /// Fetch the dynamic provider manifests. Failure keeps the last known
    /// list, records a provider-scoped error, and never flips the overall
    /// connection state.
    func fetchProviders() async {
        guard let url = URL(string: "\(baseURL)/api/providers") else {
            providerErrorMessage = "Invalid providers URL"
            return
        }
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                providerErrorMessage = "Provider server error"
                return
            }
            guard let decoded = try? JSONDecoder().decode(ProvidersResponse.self, from: data) else {
                providerErrorMessage = "Invalid provider payload"
                return
            }
            providers = decoded
            providerErrorMessage = nil
            providerLastSuccess = Date()
        } catch let urlError as URLError {
            if urlError.code == .cannotConnectToHost || urlError.code == .timedOut {
                providerErrorMessage = "Cannot connect to AICC provider API"
            } else {
                providerErrorMessage = urlError.localizedDescription
            }
        } catch {
            providerErrorMessage = error.localizedDescription
        }
    }

    /// Force-refresh one provider and then reload the whole snapshot.
    func refreshProvider(id: String) async {
        guard let path = ProviderAPI.refreshPath(providerID: id),
              let url = URL(string: "\(baseURL)\(path)") else { return }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        do {
            _ = try await session.data(for: request)
        } catch {
            return
        }
        await fetchStatus()
    }

    /// Execute a whitelisted provider action by its `kind`. The manifest `id`
    /// is only a list identifier and is never used in the route. Returns the
    /// response body for display-only actions (diagnostics), otherwise nil
    /// after a reload.
    func performProviderAction(providerId: String, kind: String) async -> String? {
        guard let path = ProviderAPI.actionPath(providerID: providerId, kind: kind),
              let url = URL(string: "\(baseURL)\(path)") else {
            return nil
        }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse else { return nil }
            let body = String(data: data, encoding: .utf8)
            guard http.statusCode == 200 else { return body }
            if kind == "diagnostics" {
                if let payload = try? JSONSerialization.jsonObject(with: data),
                   let pretty = try? JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted, .sortedKeys]) {
                    return String(data: pretty, encoding: .utf8)
                }
                return body
            }
            await fetchStatus()
            return body
        } catch {
            return nil
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
