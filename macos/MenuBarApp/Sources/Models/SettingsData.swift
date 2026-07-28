import Foundation
import SwiftUI

class AppSettings: ObservableObject {
    @AppStorage("launchAtLogin") var launchAtLogin = false
    @AppStorage("autoRefreshInterval") var autoRefreshInterval = 10.0
    @AppStorage("themeMode") var themeMode = "system"

    @AppStorage("menuBarShowCodexStatus") var menuBarShowCodexStatus = true
    @AppStorage("menuBarShowCodexBalance") var menuBarShowCodexBalance = true
    @AppStorage("menuBarShowWorkBuddy") var menuBarShowWorkBuddy = true
    @AppStorage("menuBarShowDeepSeek") var menuBarShowDeepSeek = true
    @AppStorage("menuBarShowOpenCodex") var menuBarShowOpenCodex = true
    @AppStorage("menuBarShowSystemHealth") var menuBarShowSystemHealth = true

    @AppStorage("ocxAutoStart") var ocxAutoStart = false
    @AppStorage("ocxStopOnCodexExit") var ocxStopOnCodexExit = false
    @AppStorage("ocxWaitProxy") var ocxWaitProxy = false
    @AppStorage("ocxCustomPath") var ocxCustomPath = ""
    @AppStorage("ocxServiceAddress") var ocxServiceAddress = "http://127.0.0.1:10100"

    @AppStorage("debugMode") var debugMode = false

    var preferredColorScheme: ColorScheme? {
        switch themeMode {
        case "light": return .light
        case "dark": return .dark
        default: return nil
        }
    }

    static let shared = AppSettings()
}
