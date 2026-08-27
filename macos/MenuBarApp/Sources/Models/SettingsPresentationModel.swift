import Foundation

enum AppLanguage: String, CaseIterable, Identifiable {
    case system
    case english = "en"
    case simplifiedChinese = "zh-Hans"

    var id: String { rawValue }

    var displayNameKey: String {
        switch self {
        case .system: return "System Default"
        case .english: return "English"
        case .simplifiedChinese: return "简体中文"
        }
    }

    var nativeDisplayName: String {
        switch self {
        case .system: return "System Default"
        case .english: return "English"
        case .simplifiedChinese: return "简体中文"
        }
    }

    func pickerDisplayName(localize: (String) -> String) -> String {
        switch self {
        case .system:
            return localize("System Default")
        case .english, .simplifiedChinese:
            return nativeDisplayName
        }
    }

    var locale: Locale {
        switch self {
        case .system: return .autoupdatingCurrent
        case .english: return Locale(identifier: "en")
        case .simplifiedChinese: return Locale(identifier: "zh-Hans")
        }
    }

    var localizationIdentifier: String {
        switch self {
        case .system:
            return Locale.preferredLanguages.first?.hasPrefix("zh") == true ? "zh-Hans" : "en"
        case .english: return "en"
        case .simplifiedChinese: return "zh-Hans"
        }
    }
}

enum AppThemeMode: String, CaseIterable {
    case system
    case light
    case dark
}

struct SettingsPresentationIdentity: Equatable, Hashable {
    let languageCode: String
    let themeMode: String
}

struct StatusItemDisplayState: Equatable {
    let remaining: Double?
    let showStatus: Bool
    let showBalance: Bool
    let languageCode: String
    let themeMode: String

    init(
        status: StatusResponse?,
        showStatus: Bool,
        showBalance: Bool,
        languageCode: String,
        themeMode: String
    ) {
        self.remaining = status?.codex?.weekly?.remaining ?? status?.codex?.five_hour?.remaining
        self.showStatus = showStatus
        self.showBalance = showBalance
        self.languageCode = languageCode
        self.themeMode = themeMode
    }

    init(
        remaining: Double?,
        showStatus: Bool,
        showBalance: Bool,
        languageCode: String,
        themeMode: String
    ) {
        self.remaining = remaining
        self.showStatus = showStatus
        self.showBalance = showBalance
        self.languageCode = languageCode
        self.themeMode = themeMode
    }
}

/// AppKit termination is allowed only after an explicit application-quit
/// command. Closing the last regular window must never satisfy this gate.
struct AppTerminationGate: Equatable {
    private(set) var terminationRequested = false

    mutating func requestTermination() {
        terminationRequested = true
    }

    var allowsTermination: Bool {
        terminationRequested
    }
}
