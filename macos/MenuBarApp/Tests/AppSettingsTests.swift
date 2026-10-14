import XCTest
@testable import AICCCore

final class AppSettingsTests: XCTestCase {
    func testBridgePreferenceDoesNotEnableLegacyServices() {
        let settings = AppSettings.shared
        let previous = settings.bridgeEnabled
        defer { settings.bridgeEnabled = previous }
        settings.bridgeEnabled = false
        XCTAssertFalse(UserDefaults.standard.bool(forKey: "bridgeEnabled"))
        settings.bridgeEnabled = true
        XCTAssertTrue(UserDefaults.standard.bool(forKey: "bridgeEnabled"))
    }
}
