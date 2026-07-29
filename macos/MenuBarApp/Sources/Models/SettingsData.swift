import Foundation
import SwiftUI

enum AppLanguage: String, CaseIterable, Identifiable {
    case system
    case english = "en"
    case simplifiedChinese = "zh-Hans"

    var id: String { rawValue }

    var displayName: LocalizedStringKey {
        switch self {
        case .system: return "System Default"
        case .english: return "English"
        case .simplifiedChinese: return "Simplified Chinese"
        }
    }

    var locale: Locale {
        switch self {
        case .system: return .autoupdatingCurrent
        case .english: return Locale(identifier: "en")
        case .simplifiedChinese: return Locale(identifier: "zh-Hans")
        }
    }
}

class AppSettings: ObservableObject {
    @AppStorage("launchAtLogin") var launchAtLogin = false
    @AppStorage("autoRefreshInterval") var autoRefreshInterval = 120.0
    @AppStorage("themeMode") var themeMode = "system"
    @Published var languageCode: String {
        didSet {
            UserDefaults.standard.set(languageCode, forKey: "languageCode")
        }
    }

    @AppStorage("menuBarShowCodexStatus") var menuBarShowCodexStatus = true
    @AppStorage("menuBarShowCodexBalance") var menuBarShowCodexBalance = true
    @AppStorage("menuBarShowWorkBuddy") var menuBarShowWorkBuddy = true
    @AppStorage("menuBarShowDeepSeek") var menuBarShowDeepSeek = true
    @AppStorage("menuBarShowOpenCodex") var menuBarShowOpenCodex = true

    @AppStorage("ocxCustomPath") var ocxCustomPath = ""

    @AppStorage("debugMode") var debugMode = false

    var preferredColorScheme: ColorScheme? {
        switch themeMode {
        case "light": return .light
        case "dark": return .dark
        default: return nil
        }
    }

    var language: AppLanguage {
        AppLanguage(rawValue: languageCode) ?? .system
    }

    var locale: Locale {
        language.locale
    }

    func localized(_ key: String) -> String {
        let localization: String
        switch language {
        case .system:
            localization = Locale.preferredLanguages.first?.hasPrefix("zh") == true ? "zh-Hans" : "en"
        case .english:
            localization = "en"
        case .simplifiedChinese:
            localization = "zh-Hans"
        }

        guard
            let path = Bundle.main.path(forResource: localization, ofType: "lproj"),
            let bundle = Bundle(path: path)
        else {
            return key
        }
        return bundle.localizedString(forKey: key, value: key, table: nil)
    }

    private init() {
        languageCode = UserDefaults.standard.string(forKey: "languageCode")
            ?? AppLanguage.system.rawValue
        let storedInterval = UserDefaults.standard.double(forKey: "autoRefreshInterval")
        if storedInterval < 60 {
            UserDefaults.standard.set(120.0, forKey: "autoRefreshInterval")
        }
    }

    static let shared = AppSettings()
}
