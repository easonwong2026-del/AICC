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

    // MARK: - 两个受支持的安装位置

    /// `/Applications` 与 `~/Applications` 都受支持：DMG 的快捷方式指向后者，
    /// 但没有管理员权限的机器只会用前者。只认一个目录会让另一处的副本修不了小组件。
    func testBothSupportedInstallLocationsAreSearched() {
        XCTAssertEqual(WidgetInstallation.installRoots(home: "/Users/me"),
                       ["/Applications", "/Users/me/Applications"])
    }

    /// 装在 `~/Applications` 时，转位启动必须能找到那份安装副本。
    /// 旧实现写死 `/Applications`，这里会返回 nil，小组件就一直是黑块。
    func testTheInstalledCopyIsFoundInTheUserApplicationsFolder() {
        let home = "/Users/me/Applications/AICC.app"
        let installed = WidgetInstallation.installedPath(
            forBundlePath: translocated,
            roots: WidgetInstallation.installRoots(home: "/Users/me"),
            exists: { $0 == home }
        )
        XCTAssertEqual(installed, home)
    }

    /// 两份都在时优先 `/Applications`：那是 DMG 引导的位置，也是最容易被系统注册的一份。
    func testApplicationsWinsWhenBothCopiesExist() {
        let installed = WidgetInstallation.installedPath(
            forBundlePath: translocated,
            roots: WidgetInstallation.installRoots(home: "/Users/me"),
            exists: { _ in true }
        )
        XCTAssertEqual(installed, "/Applications/AICC.app")
    }

    /// 两份都没有（例如直接从 DMG 里双击运行）：返回 nil，由调用方说明原因，
    /// 而不是拿只读的转位副本去修。
    func testNoInstalledCopyUnderEitherRootStaysNil() {
        XCTAssertNil(WidgetInstallation.installedPath(
            forBundlePath: translocated,
            roots: WidgetInstallation.installRoots(home: "/Users/me"),
            exists: { _ in false }
        ))
    }
}
