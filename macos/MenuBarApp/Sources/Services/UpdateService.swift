import Combine
import Foundation

protocol UpdateURLSession: Sendable {
    func data(for request: URLRequest) async throws -> (Data, URLResponse)
    func boundedData(for request: URLRequest, maxBytes: Int) async throws -> (Data, URLResponse)
}

extension UpdateURLSession {
    func boundedData(for request: URLRequest, maxBytes: Int) async throws -> (Data, URLResponse) {
        let (data, response) = try await data(for: request)
        if let httpResponse = response as? HTTPURLResponse,
           httpResponse.expectedContentLength > Int64(maxBytes) {
            throw UpdateServiceTransportError.manifestTooLarge
        }
        guard data.count <= maxBytes else {
            throw UpdateServiceTransportError.manifestTooLarge
        }
        return (data, response)
    }
}

enum UpdateServiceTransportError: Error, Sendable {
    case manifestTooLarge
}

extension URLSession: UpdateURLSession {
    func boundedData(for request: URLRequest, maxBytes: Int) async throws -> (Data, URLResponse) {
        let (asyncBytes, response) = try await bytes(for: request)
        if let httpResponse = response as? HTTPURLResponse,
           httpResponse.expectedContentLength > Int64(maxBytes) {
            throw UpdateServiceTransportError.manifestTooLarge
        }
        var data = Data()
        if let httpResponse = response as? HTTPURLResponse,
           httpResponse.expectedContentLength > 0 {
            data.reserveCapacity(min(Int(httpResponse.expectedContentLength), maxBytes))
        }
        for try await byte in asyncBytes {
            if data.count >= maxBytes {
                throw UpdateServiceTransportError.manifestTooLarge
            }
            data.append(byte)
        }
        return (data, response)
    }
}

private enum UpdateServiceError: Error, Sendable {
    case invalidManifestURL
    case invalidResponse
    case httpStatus(Int)
    case manifestTooLarge
    case invalidManifest
    case currentVersionUnavailable
}

final class HTTPSOnlyRedirectDelegate: NSObject, URLSessionTaskDelegate, Sendable {
    let allowedHosts: Set<String>

    init(allowedHosts: Set<String> = UpdateManifestConfiguration.defaultTrustedHosts) {
        self.allowedHosts = allowedHosts
        super.init()
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        guard
            let url = request.url,
            UpdateManifestConfiguration.isTrustedHTTPSURL(url, allowedHosts: allowedHosts)
        else {
            completionHandler(nil)
            return
        }
        completionHandler(request)
    }
}

@MainActor
final class UpdateService: ObservableObject {
    nonisolated static let maximumManifestBytes = 1_048_576
    nonisolated static let defaultAutoCheckInterval: TimeInterval = 24 * 60 * 60
    nonisolated static let lastCheckedDefaultsKey = "updateLastCheckedAt"
    nonisolated static let ignoredVersionDefaultsKey = "updateIgnoredVersion"

    static let shared = UpdateService(userDefaults: .standard)

    @Published private(set) var state: UpdateCheckState
    @Published private(set) var lastCheckedAt: Date?
    @Published private(set) var ignoredVersion: String?

    let currentVersion: AppVersionInfo
    let currentSystemVersion: SystemVersion
    let manifestURL: URL?
    let releasePageURL: URL

    private let session: UpdateURLSession
    private let ownedSession: URLSession?
    private let timeoutInterval: TimeInterval
    private let sessionDelegate: HTTPSOnlyRedirectDelegate?
    private let allowedHosts: Set<String>
    private let userDefaults: UserDefaults?
    private let autoCheckInterval: TimeInterval
    private let now: () -> Date

    /// 新版本存在且未被用户主动忽略（用于菜单栏与侧边栏的温和提醒）。
    var visibleUpdate: UpdateInfo? {
        guard case .updateAvailable(let info) = state else { return nil }
        if ignoredVersion == info.version { return nil }
        return info
    }

    /// 检测到的新版本（即使已被忽略，设置页仍可看到并一键下载）。
    var availableUpdate: UpdateInfo? {
        guard case .updateAvailable(let info) = state else { return nil }
        return info
    }

    init(
        currentVersion: AppVersionInfo = AppVersionInfo(bundle: .main),
        currentSystemVersion: SystemVersion = .current,
        manifestURL: URL? = UpdateManifestConfiguration.manifestURL(),
        releasePageURL: URL = UpdateManifestConfiguration.releasePageURL(),
        session: UpdateURLSession? = nil,
        timeoutInterval: TimeInterval = 10,
        userDefaults: UserDefaults? = nil,
        autoCheckInterval: TimeInterval = UpdateService.defaultAutoCheckInterval,
        now: @escaping () -> Date = Date.init
    ) {
        self.currentVersion = currentVersion
        self.currentSystemVersion = currentSystemVersion
        self.manifestURL = manifestURL
        self.releasePageURL = UpdateManifestConfiguration.httpsURL(releasePageURL.absoluteString)
            ?? UpdateManifestConfiguration.defaultReleasePageURL
        self.timeoutInterval = max(1, timeoutInterval)
        self.allowedHosts = UpdateManifestConfiguration.trustedHosts(for: manifestURL)
        self.userDefaults = userDefaults
        self.autoCheckInterval = max(0, autoCheckInterval)
        self.now = now
        self.state = manifestURL == nil ? .notConfigured : .idle
        if let userDefaults {
            let timestamp = userDefaults.double(forKey: Self.lastCheckedDefaultsKey)
            self.lastCheckedAt = timestamp > 0 ? Date(timeIntervalSince1970: timestamp) : nil
            let ignored = userDefaults.string(forKey: Self.ignoredVersionDefaultsKey)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            self.ignoredVersion = (ignored?.isEmpty == false) ? ignored : nil
        } else {
            self.lastCheckedAt = nil
            self.ignoredVersion = nil
        }
        if let session {
            self.session = session
            self.ownedSession = nil
            self.sessionDelegate = nil
        } else {
            let delegate = HTTPSOnlyRedirectDelegate(allowedHosts: allowedHosts)
            let created = Self.makeSession(timeoutInterval: max(1, timeoutInterval), delegate: delegate)
            self.session = created
            self.ownedSession = created
            self.sessionDelegate = delegate
        }
    }

    deinit {
        ownedSession?.invalidateAndCancel()
    }

    func ignoreUpdate(_ info: UpdateInfo) {
        ignoredVersion = info.version
        userDefaults?.set(info.version, forKey: Self.ignoredVersionDefaultsKey)
    }

    func clearIgnoredUpdate() {
        ignoredVersion = nil
        userDefaults?.removeObject(forKey: Self.ignoredVersionDefaultsKey)
    }

    func preferredDownloadURL(for info: UpdateInfo) -> URL {
        if let downloadURL = info.downloadURL,
           UpdateManifestConfiguration.httpsURL(downloadURL.absoluteString) != nil {
            return downloadURL
        }
        return preferredReleaseNotesURL(for: info)
    }

    func preferredReleaseNotesURL(for info: UpdateInfo) -> URL {
        if let notesURL = info.releaseNotesURL,
           UpdateManifestConfiguration.httpsURL(notesURL.absoluteString) != nil {
            return notesURL
        }
        return releasePageURL
    }

    /// 启动后静默检查更新：遵守开关与检查周期节流，不打扰用户。
    @discardableResult
    func checkForUpdatesIfDue(autoCheckEnabled: Bool) async -> UpdateCheckState {
        guard autoCheckEnabled else { return state }
        guard manifestURL != nil, state != .checking else { return state }
        if let lastCheckedAt, now().timeIntervalSince(lastCheckedAt) < autoCheckInterval {
            return state
        }
        return await checkForUpdates(manual: false)
    }

    @discardableResult
    func checkForUpdates(manual: Bool = true) async -> UpdateCheckState {
        guard state != .checking else { return state }

        guard let manifestURL else {
            state = .notConfigured
            return state
        }
        guard UpdateManifestConfiguration.httpsURL(manifestURL.absoluteString) != nil else {
            if manual {
                state = .failed(UpdateServiceError.invalidManifestURL.message)
            }
            return state
        }
        guard let currentSemanticVersion = SemanticVersion(currentVersion.shortVersion) else {
            if manual {
                state = .failed(UpdateServiceError.currentVersionUnavailable.message)
            }
            return state
        }

        let previousState = state
        if manual {
            clearIgnoredUpdate()
        }
        state = .checking

        do {
            let request = makeRequest(url: manifestURL)
            let (data, response) = try await session.boundedData(
                for: request,
                maxBytes: Self.maximumManifestBytes
            )
            guard data.count <= Self.maximumManifestBytes else {
                throw UpdateServiceError.manifestTooLarge
            }
            guard let httpResponse = response as? HTTPURLResponse else {
                throw UpdateServiceError.invalidResponse
            }
            guard (200..<300).contains(httpResponse.statusCode) else {
                throw UpdateServiceError.httpStatus(httpResponse.statusCode)
            }
            guard
                let responseURL = httpResponse.url,
                UpdateManifestConfiguration.isTrustedHTTPSURL(responseURL, allowedHosts: allowedHosts)
            else {
                throw UpdateServiceError.invalidManifestURL
            }

            let info: UpdateInfo
            do {
                info = try JSONDecoder().decode(UpdateInfo.self, from: data)
            } catch {
                throw UpdateServiceError.invalidManifest
            }

            guard
                let remoteVersion = SemanticVersion(info.version),
                info.downloadURL.map({ UpdateManifestConfiguration.httpsURL($0.absoluteString) != nil }) ?? true,
                info.releaseNotesURL.map({ UpdateManifestConfiguration.httpsURL($0.absoluteString) != nil }) ?? true
            else {
                throw UpdateServiceError.invalidManifest
            }

            let isNewer: Bool
            if remoteVersion > currentSemanticVersion {
                isNewer = true
            } else if remoteVersion == currentSemanticVersion,
                      let remoteBuild = info.numericBuild,
                      let currentBuild = currentVersion.numericBuild,
                      remoteBuild > currentBuild {
                isNewer = true
            } else {
                isNewer = false
            }

            if isNewer {
                if let requiredRaw = info.minimumSystemVersion {
                    guard let requiredSystem = SystemVersion(requiredRaw) else {
                        throw UpdateServiceError.invalidManifest
                    }
                    if currentSystemVersion < requiredSystem {
                        state = .incompatibleSystem(info, requiredVersion: requiredRaw)
                    } else {
                        state = .updateAvailable(info)
                    }
                } else {
                    state = .updateAvailable(info)
                }
            } else {
                state = .upToDate
            }

            let checkedNow = now()
            lastCheckedAt = checkedNow
            userDefaults?.set(checkedNow.timeIntervalSince1970, forKey: Self.lastCheckedDefaultsKey)
        } catch is CancellationError {
            if !manual || previousState.isActionableResult {
                state = previousState
            } else {
                state = .failed("已取消检查更新")
            }
        } catch UpdateServiceTransportError.manifestTooLarge {
            state = manual ? .failed(UpdateServiceError.manifestTooLarge.message) : previousState
        } catch let error as URLError where error.code == .timedOut {
            state = manual ? .failed("检查更新超时，请稍后重试") : previousState
        } catch let error as UpdateServiceError {
            state = manual ? .failed(error.message) : previousState
        } catch {
            state = manual ? .failed("检查更新失败，请检查网络后重试") : previousState
        }

        return state
    }

    private func makeRequest(url: URL) -> URLRequest {
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = timeoutInterval
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        return request
    }

    private static func makeSession(
        timeoutInterval: TimeInterval,
        delegate: URLSessionDelegate
    ) -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.urlCache = nil
        configuration.timeoutIntervalForRequest = timeoutInterval
        configuration.timeoutIntervalForResource = timeoutInterval
        return URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
    }
}

private extension UpdateCheckState {
    var isActionableResult: Bool {
        switch self {
        case .upToDate, .updateAvailable, .incompatibleSystem:
            return true
        case .idle, .checking, .failed, .notConfigured:
            return false
        }
    }
}

private extension UpdateServiceError {
    var message: String {
        switch self {
        case .invalidManifestURL:
            return "更新源地址无效"
        case .invalidResponse:
            return "更新服务器返回了无效响应"
        case .httpStatus(let status):
            return "更新服务器返回 HTTP \(status)"
        case .manifestTooLarge:
            return "更新清单体积超出限制"
        case .invalidManifest:
            return "更新清单格式无效"
        case .currentVersionUnavailable:
            return "当前应用版本不可用"
        }
    }
}
