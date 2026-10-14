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
