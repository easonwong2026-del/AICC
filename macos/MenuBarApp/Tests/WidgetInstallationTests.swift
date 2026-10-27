import XCTest
@testable import AICCCore

/// Desktop widgets in 2.12.1 rendered as a featureless black tile because the
/// App ran from an App Translocation copy: `chronod` never registers an
/// extension that lives in that random, read-only directory.  These are the
/// path rules the repair depends on.
final class WidgetInstallationTests: XCTestCase {
    private let translocated = "/private/var/folders/vm/abc/T/AppTranslocation/74B90F69-B254/d/AICC.app"

    func testATranslocatedBundleIsRecognised() {
        XCTAssertTrue(WidgetInstallation.isTranslocated(bundlePath: translocated))
    }

    func testAnInstalledBundleIsNotTranslocated() {
        XCTAssertFalse(WidgetInstallation.isTranslocated(bundlePath: "/Applications/AICC.app"))
        XCTAssertFalse(WidgetInstallation.isTranslocated(bundlePath: "/Users/me/WorkBuddy/aicc/AICC/dist/mac/AICC.app"))
    }

    func testTheInstalledCopyIsFoundForATranslocatedLaunch() {
        let installed = WidgetInstallation.installedPath(forBundlePath: translocated) { _ in true }
        XCTAssertEqual(installed, "/Applications/AICC.app")
    }

    func testANormalLaunchHasNothingToSwitchTo() {
        XCTAssertNil(WidgetInstallation.installedPath(forBundlePath: "/Applications/AICC.app") { _ in true })
    }

    func testATranslocatedBundleWithNoInstalledCopyStaysWhereItIs() {
        XCTAssertNil(WidgetInstallation.installedPath(forBundlePath: translocated) { _ in false })
    }

    func testTheInstalledCopyKeepsTheBundleName() {
        let other = "/private/var/folders/x/T/AppTranslocation/UUID/d/AICC Beta.app"
        XCTAssertEqual(WidgetInstallation.installedPath(forBundlePath: other) { _ in true },
                       "/Applications/AICC Beta.app")
    }
}
