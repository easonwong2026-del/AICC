import Foundation
import SwiftUI

class AppSettings: ObservableObject {
    @AppStorage("launchAtLogin") var launchAtLogin = false
    @AppStorage("menuBarShowIcon") var menuBarShowIcon = true
    @AppStorage("autoRefreshInterval") var autoRefreshInterval = 10.0
    @AppStorage("enableNotifications") var enableNotifications = false
    @AppStorage("themeMode") var themeMode = "system"

    @AppStorage("ocxAutoStart") var ocxAutoStart = false
    @AppStorage("ocxStopOnCodexExit") var ocxStopOnCodexExit = false
    @AppStorage("ocxWaitProxy") var ocxWaitProxy = false
    @AppStorage("ocxCustomPath") var ocxCustomPath = ""
    @AppStorage("ocxServiceAddress") var ocxServiceAddress = "http://127.0.0.1:10100"

    @AppStorage("einkSyncInterval") var einkSyncInterval = 30.0

    static let shared = AppSettings()
}
