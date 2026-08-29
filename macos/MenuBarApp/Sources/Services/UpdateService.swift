import Combine
import Foundation

protocol UpdateURLSession {
    func data(for request: URLRequest) async throws -> (Data, URLResponse)
}

extension URLSession: UpdateURLSession {}

private enum UpdateServiceError: Error {
    case invalidManifestURL
    case invalidResponse
    case httpStatus(Int)
    case manifestTooLarge
    case invalidManifest
    case currentVersionUnavailable
}

private final class HTTPSOnlyRedirectDelegate: NSObject, URLSessionTaskDelegate {
    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        guard
            let url = request.url,
            UpdateManifestConfiguration.httpsURL(url.absoluteString) != nil
        else {
            completionHandler(nil)
            return
        }
        completionHandler(request)
    }
}

@MainActor
final class UpdateService: ObservableObject {
    static let maximumManifestBytes = 1_048_576

    @Published private(set) var state: UpdateCheckState

    let currentVersion: AppVersionInfo
    let manifestURL: URL?
    let releasePageURL: URL

    private let session: UpdateURLSession
    private let timeoutInterval: TimeInterval
    private let sessionDelegate: HTTPSOnlyRedirectDelegate?

    init(
        currentVersion: AppVersionInfo = AppVersionInfo(bundle: .main),
        manifestURL: URL? = UpdateManifestConfiguration.manifestURL(),
        releasePageURL: URL = UpdateManifestConfiguration.releasePageURL(),
        session: UpdateURLSession? = nil,
        timeoutInterval: TimeInterval = 10
    ) {
        self.currentVersion = currentVersion
        self.manifestURL = manifestURL
        self.releasePageURL = UpdateManifestConfiguration.httpsURL(releasePageURL.absoluteString)
            ?? UpdateManifestConfiguration.defaultReleasePageURL
        self.timeoutInterval = max(1, timeoutInterval)
        self.state = manifestURL == nil ? .notConfigured : .idle
        if let session {
            self.session = session
            self.sessionDelegate = nil
        } else {
            let delegate = HTTPSOnlyRedirectDelegate()
            self.session = Self.makeSession(timeoutInterval: max(1, timeoutInterval), delegate: delegate)
            self.sessionDelegate = delegate
        }
    }

    @discardableResult
    func checkForUpdates() async -> UpdateCheckState {
        guard state != .checking else { return state }

        guard let manifestURL else {
            state = .notConfigured
            return state
        }
        guard UpdateManifestConfiguration.httpsURL(manifestURL.absoluteString) != nil else {
            state = .failed("Invalid update source.")
            return state
        }

        state = .checking

        do {
            let request = makeRequest(url: manifestURL)
            let (data, response) = try await session.data(for: request)
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
                UpdateManifestConfiguration.httpsURL(responseURL.absoluteString) != nil
            else {
                throw UpdateServiceError.invalidManifestURL
            }

            let info: UpdateInfo
            do {
                info = try JSONDecoder().decode(UpdateInfo.self, from: data)
            } catch {
                throw UpdateServiceError.invalidManifest
            }

            guard let currentSemanticVersion = SemanticVersion(self.currentVersion.shortVersion) else {
                throw UpdateServiceError.currentVersionUnavailable
            }
            guard
                let remoteVersion = SemanticVersion(info.version),
                info.downloadURL.map({ UpdateManifestConfiguration.httpsURL($0.absoluteString) != nil }) ?? true,
                info.releaseNotesURL.map({ UpdateManifestConfiguration.httpsURL($0.absoluteString) != nil }) ?? true
            else {
                throw UpdateServiceError.invalidManifest
            }

            state = remoteVersion > currentSemanticVersion ? .updateAvailable(info) : .upToDate
        } catch is CancellationError {
            state = .failed("Update check cancelled.")
        } catch let error as URLError where error.code == .timedOut {
            state = .failed("Update check timed out.")
        } catch let error as UpdateServiceError {
            state = .failed(error.message)
        } catch {
            state = .failed("Update check failed.")
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
        configuration.timeoutIntervalForRequest = timeoutInterval
        configuration.timeoutIntervalForResource = timeoutInterval
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
    }
}

private extension UpdateServiceError {
    var message: String {
        switch self {
        case .invalidManifestURL:
            return "Invalid update source."
        case .invalidResponse:
            return "Invalid update response."
        case .httpStatus(let status):
            return "Update server returned HTTP \(status)."
        case .manifestTooLarge, .invalidManifest:
            return "Invalid update manifest."
        case .currentVersionUnavailable:
            return "Current app version is unavailable."
        }
    }
}
