import XCTest
@testable import AICCCore

/// 操作反馈的文案契约。
///
/// 这些断言防的是"把没发生的事说成发生了"：没开始说成已开始、没暂停说成已暂停、
/// 关闭了却没说账号回没回来。
final class WorkbuddyRotationFeedbackTests: XCTestCase {
    func testAlreadyRanAndSkippedAreNotReportedAsStarted() {
        let already = RotationFeedbackText.runNow(status: "already_ran", detail: nil)
        XCTAssertFalse(already.isFailure)
        XCTAssertFalse(already.text.contains("已开始"), already.text)
        XCTAssertTrue(already.text.contains("今天已经执行过"), already.text)

        let skipped = RotationFeedbackText.runNow(status: "skipped", detail: "窗口不足")
        XCTAssertTrue(skipped.isFailure)
        XCTAssertFalse(skipped.text.contains("已开始"), skipped.text)
        XCTAssertTrue(skipped.text.contains("窗口不足"), skipped.text)

        let busy = RotationFeedbackText.runNow(status: "busy", detail: nil)
        XCTAssertFalse(busy.text.contains("已开始"), busy.text)
    }

    func testOnlyRealProgressSaysItStarted() {
        for status in ["prechecking", "switching", "dwelling", "restoring"] {
            let feedback = RotationFeedbackText.runNow(status: status, detail: nil)
            XCTAssertTrue(feedback.text.contains("已开始"), "\(status): \(feedback.text)")
            XCTAssertFalse(feedback.isFailure)
        }

        let completed = RotationFeedbackText.runNow(status: "completed", detail: nil)
        XCTAssertTrue(completed.text.contains("恢复"), completed.text)
    }

    func testBusyStopIsNotReportedAsPaused() {
        let busy = RotationFeedbackText.stop(status: "busy", restored: nil, detail: nil)
        XCTAssertTrue(busy.isFailure)
        XCTAssertTrue(busy.text.contains("尚未暂停"), busy.text)
        XCTAssertFalse(busy.text.contains("已暂停"), busy.text)

        let done = RotationFeedbackText.stop(status: "completed", restored: true, detail: nil)
        XCTAssertFalse(done.isFailure)
        XCTAssertTrue(done.text.contains("已暂停"), done.text)
        XCTAssertTrue(done.text.contains("恢复"), done.text)
    }

    func testTurningTheSwitchOffSeparatesStoppedFromRestored() {
        let busy = RotationFeedbackText.toggle(enabled: false, stopStatus: "busy", restored: nil)
        XCTAssertTrue(busy.text.contains("计划已关闭"), busy.text)
        XCTAssertTrue(busy.text.contains("结束后会自动恢复"), busy.text)

        let restored = RotationFeedbackText.toggle(enabled: false, stopStatus: "completed", restored: true)
        XCTAssertTrue(restored.text.contains("已关闭"), restored.text)
        XCTAssertTrue(restored.text.contains("已恢复原账号"), restored.text)
        XCTAssertFalse(restored.isFailure)

        let failed = RotationFeedbackText.toggle(enabled: false, stopStatus: "recovery_required", restored: false)
        XCTAssertTrue(failed.isFailure)
        XCTAssertTrue(failed.text.contains("没能自动恢复"), failed.text)

        let nothing = RotationFeedbackText.toggle(enabled: false, stopStatus: "nothing_to_stop", restored: nil)
        XCTAssertTrue(nothing.text.contains("已关闭"), nothing.text)
    }

    func testUnsavedEditsBlockPreviewAndRun() {
        XCTAssertFalse(RotationFeedbackText.canPreview(dirty: true, busy: false))
        XCTAssertFalse(RotationFeedbackText.canRunNow(dirty: true, busy: false))
        XCTAssertTrue(RotationFeedbackText.canPreview(dirty: false, busy: false))
        XCTAssertTrue(RotationFeedbackText.canRunNow(dirty: false, busy: false))
        XCTAssertFalse(RotationFeedbackText.canRunNow(dirty: false, busy: true))
    }

    /// 保存失败就不能继续开启：否则会用一个用户以为已经改过、实际没保存的旧计划。
    func testAFailedSaveMustNotEnableThePlan() {
        XCTAssertFalse(RotationFeedbackText.shouldApplyEnableAfterSave(saveSucceeded: false))
        XCTAssertTrue(RotationFeedbackText.shouldApplyEnableAfterSave(saveSucceeded: true))
    }
}
