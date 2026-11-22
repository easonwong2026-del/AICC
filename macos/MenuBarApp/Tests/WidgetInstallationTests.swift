import XCTest
@testable import AICCCore

/// Desktop widgets in 2.12.1 rendered as a featureless black tile because the
/// App ran from an App Translocation copy: `chronod` never registers an
/// extension that lives in that random, read-only directory.  These are the
/// rules the repair depends on — including the one that decides *which* copy is
/// the right one to touch.
final class WidgetInstallationTests: XCTestCase {
    private let translocated = "/private/var/folders/vm/abc/T/AppTranslocation/74B90F69-B254/d/AICC.app"
    private let systemCopy = "/Applications/AICC.app"
    private let userCopy = "/Users/me/Applications/AICC.app"

    // MARK: - 转位识别

    func testATranslocatedBundleIsRecognised() {
        XCTAssertTrue(WidgetInstallation.isTranslocated(bundlePath: translocated))
    }

    func testAnInstalledBundleIsNotTranslocated() {
        XCTAssertFalse(WidgetInstallation.isTranslocated(bundlePath: "/Applications/AICC.app"))
        XCTAssertFalse(WidgetInstallation.isTranslocated(bundlePath: "/Users/me/WorkBuddy/aicc/AICC/dist/mac/AICC.app"))
    }

    // MARK: - 两个受支持的安装位置

    /// `/Applications` 与 `~/Applications` 都受支持：DMG 的快捷方式指向后者，
    /// 但没有管理员权限的机器只会用前者。只认一个目录会让另一处的副本修不了小组件。
    func testBothSupportedInstallLocationsAreSearched() {
        XCTAssertEqual(WidgetInstallation.installRoots(home: "/Users/me"),
                       ["/Applications", "/Users/me/Applications"])
    }

    func testCandidatesFollowRootOrder() {
        let candidates = WidgetInstallation.candidates(
            forBundlePath: translocated,
            roots: WidgetInstallation.installRoots(home: "/Users/me"),
            exists: { _ in true }
        )
        XCTAssertEqual(candidates, [systemCopy, userCopy])
    }

    // MARK: - 身份匹配（PR #123 P1-1）

    /// 装在 `~/Applications` 时，转位启动必须能找到那份安装副本。
    func testTheInstalledCopyIsFoundInTheUserApplicationsFolder() {
        let installed = resolve(existing: [userCopy], identities: [userCopy: identity("aaaa")])
        XCTAssertEqual(installed, userCopy)
    }

    /// **两个位置都有 AICC，但版本不同**：运行的是 `~/Applications` 的新版，
    /// `/Applications` 还留着旧版。必须选新版，绝不能按路径优先级打到旧版上。
    func testADifferentBuildInApplicationsIsNeverChosen() {
        let installed = resolve(
            existing: [systemCopy, userCopy],
            identities: [systemCopy: identity("0000"), userCopy: identity("aaaa")]
        )
        XCTAssertEqual(installed, userCopy)
    }

    /// 反过来同样成立：运行的是 `/Applications` 的新版，`~/Applications` 是旧版。
    func testADifferentBuildInTheUserFolderIsNeverChosen() {
        let installed = resolve(
            existing: [systemCopy, userCopy],
            identities: [systemCopy: identity("aaaa"), userCopy: identity("0000")]
        )
        XCTAssertEqual(installed, systemCopy)
    }

    /// 两个位置都不是这个构建（都被换掉了）：宁可不修，也不打到错的副本上。
    func testNoCopyMatchingThisBuildMeansNothingIsRepaired() {
        XCTAssertNil(resolve(
            existing: [systemCopy, userCopy],
            identities: [systemCopy: identity("0000"), userCopy: identity("bbbb")]
        ))
    }

    /// 副本的签名读不出来（被改坏了 / 没签名）：同样不选。
    func testAnUnverifiableCopyIsNeverChosen() {
        XCTAssertNil(resolve(existing: [systemCopy, userCopy], identities: [:]))
    }

    /// 两份完全一致时，按根顺序取第一份，行为可预期。
    func testByteIdenticalCopiesFallBackToRootOrder() {
        let installed = resolve(
            existing: [systemCopy, userCopy],
            identities: [systemCopy: identity("aaaa"), userCopy: identity("aaaa")]
        )
        XCTAssertEqual(installed, systemCopy)
    }

    func testANormalLaunchHasNothingToSwitchTo() {
        XCTAssertNil(WidgetInstallation.installedPath(
            forBundlePath: systemCopy,
            identityAt: { _ in self.identity("aaaa") },
            exists: { _ in true }
        ))
    }

    func testATranslocatedBundleWithNoInstalledCopyStaysWhereItIs() {
        XCTAssertNil(resolve(existing: [], identities: [:]))
    }

    func testTheInstalledCopyKeepsTheBundleName() {
        let other = "/private/var/folders/x/T/AppTranslocation/UUID/d/AICC Beta.app"
        let beta = "/Applications/AICC Beta.app"
        let installed = WidgetInstallation.installedPath(
            forBundlePath: other,
            roots: WidgetInstallation.installRoots(home: "/Users/me"),
            identityAt: { _ in self.identity("aaaa") },
            exists: { $0 == beta }
        )
        XCTAssertEqual(installed, beta)
    }

    // MARK: - 修复计划（PR #123 P1-2）

    func testAnAlreadyRepairedBuildIsSkipped() {
        let plan = WidgetHealthService.plan(
            runningPath: systemCopy,
            recordedKey: "2.14.0/26@aaaa",
            currentKey: "2.14.0/26@aaaa",
            identityAt: { _ in self.identity("aaaa") },
            exists: { _ in true }
        )
        XCTAssertEqual(plan, .alreadyRepaired)
    }

    func testANormalLaunchRepairsItsOwnBundle() {
        let plan = WidgetHealthService.plan(
            runningPath: systemCopy,
            recordedKey: nil,
            currentKey: "2.14.0/26@aaaa",
            identityAt: { _ in self.identity("aaaa") },
            exists: { _ in true }
        )
        XCTAssertEqual(plan, .repair(path: systemCopy))
    }

    /// 转位且找不到「同一个构建」的安装副本：不修，而且原因里要写清看了哪些路径、
    /// 每份副本是什么身份，否则黑块永远是黑盒。
    func testTranslocatedWithoutAMatchingCopyRepairsNothingAndSaysWhy() {
        let plan = WidgetHealthService.plan(
            runningPath: translocated,
            recordedKey: nil,
            currentKey: "2.14.0/26@aaaa",
            roots: ["/Applications", "/Users/me/Applications"],
            identityAt: { path in path == self.systemCopy ? self.identity("0000") : self.identity("aaaa") },
            exists: { $0 == self.systemCopy }
        )
        guard case .nothingToRepair(let reason) = plan else {
            return XCTFail("不能对身份不一致的副本执行修复：\(plan)")
        }
        XCTAssertTrue(reason.contains("translocated"), reason)
        XCTAssertTrue(reason.contains(self.systemCopy), reason)
        XCTAssertTrue(reason.contains("2.13.0/25"), "原因里要带候选副本的身份：\(reason)")
    }

    /// 转位但两份都在且其中一份就是当前构建：修那一份。
    func testTranslocatedRepairsTheVerifiedCopyOnly() {
        let plan = WidgetHealthService.plan(
            runningPath: translocated,
            recordedKey: nil,
            currentKey: "2.14.0/26@aaaa",
            roots: ["/Applications", "/Users/me/Applications"],
            identityAt: { path in path == self.systemCopy ? self.identity("0000") : self.identity("aaaa") },
            exists: { _ in true }
        )
        XCTAssertEqual(plan, .repair(path: userCopy))
    }

    /// 连自己的身份都取不到：不修，也不写「已修复」。
    func testAnUnidentifiableBuildRecordsNothing() {
        let plan = WidgetHealthService.plan(
            runningPath: systemCopy,
            recordedKey: nil,
            currentKey: "",
            identityAt: { _ in nil },
            exists: { _ in true }
        )
        guard case .nothingToRepair = plan else {
            return XCTFail("空 key 不能进入修复：\(plan)")
        }
    }

    /// 只有两步都成功才算修好；`killall chronod` 不参与这个判断。
    func testRepairCountsOnlyWhenBothStepsSucceed() {
        XCTAssertTrue(WidgetHealthService.marksRepairComplete(clearedQuarantine: 0, registeredExtension: 0))
        XCTAssertFalse(WidgetHealthService.marksRepairComplete(clearedQuarantine: 1, registeredExtension: 0))
        XCTAssertFalse(WidgetHealthService.marksRepairComplete(clearedQuarantine: 0, registeredExtension: 1))
        XCTAssertFalse(WidgetHealthService.marksRepairComplete(clearedQuarantine: -1, registeredExtension: -1))
    }

    /// 同版本、同 Build 重新构建 → cdhash 变了，修复键必须跟着变，否则会被当成
    /// 「已经修过」直接跳过，pluginkit 里留的还是旧注册。
    func testARebuildOfTheSameVersionGetsAFreshRepairKey() {
        let first = WidgetHealthService.repairKey(version: "2.14.0", build: "26", cdhash: "aaaaaaaa1111")
        let second = WidgetHealthService.repairKey(version: "2.14.0", build: "26", cdhash: "bbbbbbbb2222")
        XCTAssertNotEqual(first, second)
        XCTAssertEqual(first, "2.14.0/26@aaaaaaaa1111")
    }

    /// 没有 cdhash 时退回版本号，不要退化成空串（空串等于永远不修）。
    func testRepairKeyFallsBackToVersionAndBuild() {
        XCTAssertEqual(WidgetHealthService.repairKey(version: "2.14.0", build: "26", cdhash: nil),
                       "2.14.0/26")
        XCTAssertEqual(WidgetHealthService.repairKey(version: "", build: "26", cdhash: "aaaa"), "")
    }

    // MARK: - 辅助

    private func resolve(existing: [String], identities: [String: InstalledAppIdentity]) -> String? {
        let running = identity("aaaa")
        return WidgetInstallation.installedPath(
            forBundlePath: translocated,
            roots: WidgetInstallation.installRoots(home: "/Users/me"),
            identityAt: { path in
                if path == self.translocated { return running }
                return identities[path]
            },
            exists: { existing.contains($0) }
        )
    }

    /// 身份只由 cdhash 决定；版本号只是为了日志好读。
    private func identity(_ hash: String) -> InstalledAppIdentity {
        let old = hash.hasPrefix("0000")
        return InstalledAppIdentity(
            bundleIdentifier: "com.aieink.dashboard.menubar",
            executableCDHash: hash,
            version: old ? "2.13.0" : "2.14.0",
            build: old ? "25" : "26"
        )
    }
}
