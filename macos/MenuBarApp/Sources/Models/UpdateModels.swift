import Foundation

struct AppVersionInfo: Equatable, Sendable {
    static let fallbackValue = "—"

    let shortVersion: String
    let build: String

    var version: String { shortVersion }
    var buildVersion: String { build }

    var numericBuild: Int? {
        guard let parsed = Int(build), parsed > 0 else { return nil }
        return parsed
    }

    var displayText: String {
        guard shortVersion != Self.fallbackValue else { return Self.fallbackValue }
        if build != Self.fallbackValue {
            return "v\(shortVersion) (Build \(build))"
        }
        return "v\(shortVersion)"
    }

    init(shortVersion: String?, build: String?) {
        self.shortVersion = Self.normalized(shortVersion)
        self.build = Self.normalized(build)
    }

    init(bundle: Bundle = .main) {
        self.init(
            shortVersion: bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
            build: bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        )
    }

    init(infoDictionary: [String: Any]) {
        self.init(
            shortVersion: infoDictionary["CFBundleShortVersionString"] as? String,
            build: infoDictionary["CFBundleVersion"] as? String
        )
    }

    private static func normalized(_ value: String?) -> String {
        guard let value else { return fallbackValue }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? fallbackValue : trimmed
    }
}

struct SystemVersion: Comparable, CustomStringConvertible, Equatable, Hashable, Sendable {
    let major: Int
    let minor: Int
    let patch: Int

    init(major: Int, minor: Int = 0, patch: Int = 0) {
        self.major = max(0, major)
        self.minor = max(0, minor)
        self.patch = max(0, patch)
    }

    init(_ version: OperatingSystemVersion) {
        self.init(
            major: version.majorVersion,
            minor: version.minorVersion,
            patch: version.patchVersion
        )
    }

    init?(_ rawValue: String) {
        let trimmed = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        let parts = trimmed.split(separator: ".", omittingEmptySubsequences: false)
        guard (1...3).contains(parts.count) else { return nil }
        var numbers: [Int] = []
        for part in parts {
            guard !part.isEmpty,
                  part.unicodeScalars.allSatisfy({ (48...57).contains($0.value) }),
                  let number = Int(part) else {
                return nil
            }
            numbers.append(number)
        }
        self.major = numbers[0]
        self.minor = numbers.count > 1 ? numbers[1] : 0
        self.patch = numbers.count > 2 ? numbers[2] : 0
    }

    static var current: SystemVersion {
        SystemVersion(ProcessInfo.processInfo.operatingSystemVersion)
    }

    var description: String {
        patch > 0 ? "\(major).\(minor).\(patch)" : "\(major).\(minor)"
    }

    static func < (lhs: SystemVersion, rhs: SystemVersion) -> Bool {
        if lhs.major != rhs.major { return lhs.major < rhs.major }
        if lhs.minor != rhs.minor { return lhs.minor < rhs.minor }
        return lhs.patch < rhs.patch
    }
}

struct SemanticVersion: Comparable, Codable, CustomStringConvertible, Equatable, Hashable, Sendable {
    private enum PrereleaseIdentifier: Equatable, Hashable, Sendable {
        case numeric(String)
        case text(String)
    }

    let major: Int
    let minor: Int
    let patch: Int

    private let prerelease: [PrereleaseIdentifier]
    private let normalizedDescription: String

    init?(_ rawValue: String) {
        var value = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if value.hasPrefix("v") || value.hasPrefix("V") {
            value.removeFirst()
        }
        guard !value.isEmpty else { return nil }

        let buildParts = value.split(separator: "+", maxSplits: 1, omittingEmptySubsequences: false)
        guard buildParts.count <= 2 else { return nil }
        if buildParts.count == 2 {
            let metadata = buildParts[1].split(separator: ".", omittingEmptySubsequences: false)
            guard !metadata.isEmpty, metadata.allSatisfy(Self.isValidIdentifier) else { return nil }
        }

        let coreAndPrerelease = String(buildParts[0])
        let versionParts = coreAndPrerelease.split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
        guard let corePart = versionParts.first else { return nil }

        let core = corePart.split(separator: ".", omittingEmptySubsequences: false)
        guard core.count == 3 else { return nil }
        guard let major = Self.parseCoreNumber(core[0]),
              let minor = Self.parseCoreNumber(core[1]),
              let patch = Self.parseCoreNumber(core[2]) else {
            return nil
        }

        var prereleaseIdentifiers: [PrereleaseIdentifier] = []
        if versionParts.count == 2 {
            let prereleasePart = versionParts[1]
            let identifiers = prereleasePart.split(separator: ".", omittingEmptySubsequences: false)
            guard !identifiers.isEmpty, identifiers.allSatisfy(Self.isValidIdentifier) else { return nil }
            for identifier in identifiers {
                if Self.isAllDigits(identifier) {
                    // SemVer 2.0.0 §9: numeric prerelease identifiers MUST NOT include leading zeroes.
                    if identifier.count > 1 && identifier.first == "0" {
                        return nil
                    }
                    prereleaseIdentifiers.append(.numeric(String(identifier)))
                } else {
                    prereleaseIdentifiers.append(.text(String(identifier)))
                }
            }
        }

        self.major = major
        self.minor = minor
        self.patch = patch
        self.prerelease = prereleaseIdentifiers

        let coreDescription = "\(major).\(minor).\(patch)"
        if prereleaseIdentifiers.isEmpty {
            self.normalizedDescription = coreDescription
        } else {
            let prereleaseDescription = prereleaseIdentifiers.map { identifier -> String in
                switch identifier {
                case .numeric(let value), .text(let value): return value
                }
            }.joined(separator: ".")
            self.normalizedDescription = "\(coreDescription)-\(prereleaseDescription)"
        }
    }

    var description: String { normalizedDescription }

    static func == (lhs: SemanticVersion, rhs: SemanticVersion) -> Bool {
        lhs.major == rhs.major
            && lhs.minor == rhs.minor
            && lhs.patch == rhs.patch
            && lhs.prerelease == rhs.prerelease
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(major)
        hasher.combine(minor)
        hasher.combine(patch)
        hasher.combine(prerelease)
    }

    static func < (lhs: SemanticVersion, rhs: SemanticVersion) -> Bool {
        if lhs.major != rhs.major { return lhs.major < rhs.major }
        if lhs.minor != rhs.minor { return lhs.minor < rhs.minor }
        if lhs.patch != rhs.patch { return lhs.patch < rhs.patch }

        if lhs.prerelease.isEmpty && rhs.prerelease.isEmpty { return false }
        if lhs.prerelease.isEmpty { return false }
        if rhs.prerelease.isEmpty { return true }

        let count = min(lhs.prerelease.count, rhs.prerelease.count)
        for index in 0..<count {
            let comparison = Self.comparePrereleaseIdentifier(lhs.prerelease[index], rhs.prerelease[index])
            if comparison != .orderedSame {
                return comparison == .orderedAscending
            }
        }
        return lhs.prerelease.count < rhs.prerelease.count
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let value = try container.decode(String.self)
        guard let parsed = Self(value) else {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "Invalid semantic version"
            )
        }
        self = parsed
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(description)
    }

    private static func parseCoreNumber(_ component: Substring) -> Int? {
        guard isAllDigits(component) else { return nil }
        if component.count > 1 && component.first == "0" { return nil }
        return Int(component)
    }

    private static func isAllDigits(_ value: Substring) -> Bool {
        !value.isEmpty && value.unicodeScalars.allSatisfy { scalar in
            (48...57).contains(scalar.value)
        }
    }

    private static func isValidIdentifier(_ value: Substring) -> Bool {
        !value.isEmpty && value.unicodeScalars.allSatisfy { scalar in
            let code = scalar.value
            return (48...57).contains(code)
                || (65...90).contains(code)
                || (97...122).contains(code)
                || code == 45
        }
    }

    private static func comparePrereleaseIdentifier(
        _ lhs: PrereleaseIdentifier,
        _ rhs: PrereleaseIdentifier
    ) -> ComparisonResult {
        switch (lhs, rhs) {
        case (.numeric(let lhsValue), .numeric(let rhsValue)):
            if lhsValue.count != rhsValue.count {
                return lhsValue.count < rhsValue.count ? .orderedAscending : .orderedDescending
            }
            if lhsValue == rhsValue { return .orderedSame }
            return lhsValue < rhsValue ? .orderedAscending : .orderedDescending
        case (.numeric, .text):
            return .orderedAscending
        case (.text, .numeric):
            return .orderedDescending
        case (.text(let lhsValue), .text(let rhsValue)):
            if lhsValue == rhsValue { return .orderedSame }
            return lhsValue < rhsValue ? .orderedAscending : .orderedDescending
        }
    }
}

struct UpdateInfo: Codable, Equatable, Sendable {
    let version: String
    let build: String?
    let minimumSystemVersion: String?
    let downloadURL: URL?
    let releaseNotesURL: URL?
    let publishedAt: String?
    let sha256: String?
    let releaseSummary: String?

    var numericBuild: Int? {
        guard let build, let parsed = Int(build), parsed > 0 else { return nil }
        return parsed
    }

    var displayVersion: String {
        if let build, !build.isEmpty {
            return "v\(version) (Build \(build))"
        }
        return "v\(version)"
    }

    var formattedPublishedDate: String? {
        guard let publishedAt, !publishedAt.isEmpty else { return nil }
        let iso = ISO8601DateFormatter()
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let parsed = iso.date(from: publishedAt) ?? ISO8601DateFormatter().date(from: publishedAt)
        guard let parsed else { return nil }
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: parsed)
    }

    init(
        version: String,
        build: String? = nil,
        minimumSystemVersion: String? = nil,
        downloadURL: URL? = nil,
        releaseNotesURL: URL? = nil,
        publishedAt: String? = nil,
        sha256: String? = nil,
        releaseSummary: String? = nil
    ) {
        self.version = version
        self.build = build
        self.minimumSystemVersion = minimumSystemVersion
        self.downloadURL = downloadURL
        self.releaseNotesURL = releaseNotesURL
        self.publishedAt = publishedAt
        self.sha256 = sha256
        self.releaseSummary = releaseSummary
    }

    private enum CodingKeys: String, CodingKey {
        case version
        case build
        case minimumSystemVersion
        case downloadURL
        case releaseNotesURL
        case publishedAt
        case sha256
        case releaseSummary
    }

    init(from decoder: Decoder) throws {
        // Forward-compatible decoding: ignore unknown future keys (e.g. edSignature,
        // rollout metadata) so older clients can safely read newer manifests.
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let version = try container.decode(String.self, forKey: .version)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !version.isEmpty else {
            throw DecodingError.dataCorruptedError(
                forKey: .version,
                in: container,
                debugDescription: "Update manifest version is empty"
            )
        }

        self.version = version
        self.build = try Self.decodeOptionalString(container, key: .build)
        self.minimumSystemVersion = try Self.decodeOptionalString(container, key: .minimumSystemVersion)
        self.downloadURL = try Self.decodeOptionalHTTPSURL(container, key: .downloadURL)
        self.releaseNotesURL = try Self.decodeOptionalHTTPSURL(container, key: .releaseNotesURL)
        self.publishedAt = try Self.decodeOptionalString(container, key: .publishedAt)
        self.sha256 = try Self.decodeOptionalString(container, key: .sha256)
        self.releaseSummary = try Self.decodeOptionalString(container, key: .releaseSummary)
    }

    private static func decodeOptionalString(
        _ container: KeyedDecodingContainer<CodingKeys>,
        key: CodingKeys
    ) throws -> String? {
        guard let value = try container.decodeIfPresent(String.self, forKey: key) else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func decodeOptionalHTTPSURL(
        _ container: KeyedDecodingContainer<CodingKeys>,
        key: CodingKeys
    ) throws -> URL? {
        guard let value = try container.decodeIfPresent(String.self, forKey: key) else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        guard let url = UpdateManifestConfiguration.httpsURL(trimmed) else {
            throw DecodingError.dataCorruptedError(
                forKey: key,
                in: container,
                debugDescription: "Update URLs must use HTTPS"
            )
        }
        return url
    }
}

enum UpdateCheckState: Equatable, Sendable {
    case idle
    case checking
    case upToDate
    case updateAvailable(UpdateInfo)
    case incompatibleSystem(UpdateInfo, requiredVersion: String)
    case failed(String)
    case notConfigured
}

enum UpdateManifestConfiguration {
    static let manifestInfoPlistKey = "AICCUpdateManifestURL"
    static let releasePageInfoPlistKey = "AICCReleasePageURL"
    static let environmentKey = "AICC_UPDATE_MANIFEST_URL"

    static let defaultReleasePageURL = URL(string: "https://github.com/easonwong2026-del/AICC/releases")!

    static let defaultTrustedHosts: Set<String> = [
        "github.com",
        "raw.githubusercontent.com",
        "objects.githubusercontent.com",
        "release-assets.githubusercontent.com",
        "codeload.github.com",
    ]

    #if DEBUG
    static let defaultAllowEnvironmentOverride = true
    #else
    static let defaultAllowEnvironmentOverride = false
    #endif

    static func manifestURL(
        bundle: Bundle = .main,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        allowEnvironmentOverride: Bool = defaultAllowEnvironmentOverride
    ) -> URL? {
        let environmentValue = allowEnvironmentOverride
            ? environment[environmentKey].flatMap(Self.nonEmptyString)
            : nil
        let plistValue = (bundle.object(forInfoDictionaryKey: manifestInfoPlistKey) as? String)
            .flatMap(Self.nonEmptyString)
        return (environmentValue ?? plistValue).flatMap(Self.httpsURL)
    }

    static func releasePageURL(bundle: Bundle = .main) -> URL {
        guard
            let value = bundle.object(forInfoDictionaryKey: releasePageInfoPlistKey) as? String,
            let url = httpsURL(value)
        else {
            return defaultReleasePageURL
        }
        return url
    }

    static func httpsURL(_ value: String) -> URL? {
        guard
            let url = URL(string: value.trimmingCharacters(in: .whitespacesAndNewlines)),
            url.scheme?.lowercased() == "https",
            let host = url.host,
            !host.isEmpty,
            url.user == nil,
            url.password == nil
        else {
            return nil
        }
        return url
    }

    static func trustedHosts(for manifestURL: URL?) -> Set<String> {
        var hosts = defaultTrustedHosts
        if let host = manifestURL?.host?.lowercased(), !host.isEmpty {
            hosts.insert(host)
        }
        return hosts
    }

    static func isTrustedHTTPSURL(_ url: URL, allowedHosts: Set<String>) -> Bool {
        guard let validated = httpsURL(url.absoluteString),
              let host = validated.host?.lowercased() else {
            return false
        }
        return allowedHosts.contains(host)
    }

    private static func nonEmptyString(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
