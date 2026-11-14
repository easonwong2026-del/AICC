import XCTest
@testable import AICCCore

/// PR #75：Credits 的展示模型。
///
/// 这组用例守的是**语义**，不是格式：奖励不是余额、未知不是 0、
/// 未支持不是失败、领取成功必须由服务端确认。
final class QoderCreditsModelsTests: XCTestCase {
    func testClaimableReportCarriesTheRewardSeparately() throws {
        let report = QoderCreditsReport(json: [
            "accountId": "qa_1",
            "state": "claimable",
            "claimable": 1,
            "claimed": 0,
            "supported": true,
            "today": ["campaignId": "c1", "credits": 100, "claimStatus": "CLAIMABLE"],
        ])
        let info = try XCTUnwrap(report.info)
        XCTAssertEqual(info.state, "claimable")
        XCTAssertTrue(info.canClaim)
        XCTAssertEqual(info.todayCredits, 100)
        XCTAssertEqual(info.summary, "今日可领取 100 Credits")
        XCTAssertTrue(report.isInformative)
        // 不虚构的字段保持 nil。
        XCTAssertNil(report.errorMessage)
        XCTAssertNil(report.after)
    }

    func testUnknownRewardIsNilNotZero() throws {
        let info = try XCTUnwrap(QoderCreditsInfo(json: [
            "state": "already",
            "supported": true,
            "today": ["campaignId": "c1"],
        ]))
        // 服务端没给数量：不能说"已领取 0"。
        XCTAssertNil(info.todayCredits)
        XCTAssertEqual(info.summary, "今日已领取")
        XCTAssertFalse(info.canClaim)
    }

    func testUnsupportedIsNotAFailure() throws {
        let report = QoderCreditsReport(json: [
            "accountId": "qa_2",
            "state": "unsupported",
            "supported": false,
            "error": ["kind": "unsupported", "message": "International 没有已证实的接口"],
        ])
        let info = try XCTUnwrap(report.info)
        XCTAssertEqual(info.state, "unsupported")
        XCTAssertFalse(info.canClaim)
        XCTAssertTrue(info.summary.contains("暂未支持"))
        // 未支持不是"可重试的错误"，所以 isInformative 为 false、
        // 但错误文案要能原样显示给用户。
        XCTAssertFalse(report.isInformative)
        XCTAssertEqual(report.errorMessage, "International 没有已证实的接口")
    }

    func testClaimSuccessRequiresServerConfirmation() throws {
        let report = QoderCreditsReport(json: [
            "accountId": "qa_1",
            "state": "done",
            "claimed": true,
            "after": ["state": "already", "supported": true,
                      "today": ["campaignId": "c1", "credits": 100]],
        ])
        XCTAssertEqual(report.claimed, true)
        let after = try XCTUnwrap(report.after)
        XCTAssertEqual(after.state, "already")
        XCTAssertEqual(after.todayCredits, 100)
        XCTAssertEqual(report.claimText, "已领取 100 Credits（服务端已确认）")
    }

    func testClaimThatServerDidNotConfirmSaysSo() {
        // 请求成功发出、但服务端没给 `after`：文案不许写成"已领取"。
        let report = QoderCreditsReport(json: [
            "accountId": "qa_1",
            "state": "failed",
            "claimed": false,
            "error": ["kind": "network", "message": "请求超时"],
        ])
        XCTAssertEqual(report.claimed, false)
        XCTAssertNil(report.after)
        XCTAssertEqual(report.claimText, "请求超时")
    }

    /// 并发时第二个调用会拿到 `busy`：这**不是失败**，文案不能说"领取失败"。
    func testBusyIsAWaitingStateNotAFailure() {
        let report = QoderCreditsReport(json: [
            "accountId": "qa_1",
            "state": "busy",
            "claimed": false,
            "error": ["kind": "busy", "message": "这个账号正在领取，请稍后再试"],
        ])
        XCTAssertEqual(report.claimText, "这个账号正在领取，请稍后再试")
        XCTAssertEqual(report.info?.summary, "这个账号正在领取，稍后再试")
        XCTAssertFalse(report.isInformative, "busy 不属于「有数可看」")
    }

    func testRunsKeepFailuresVisible() throws {
        let failed = try XCTUnwrap(QoderCreditRun(json: [
            "id": 7,
            "accountId": "qa_1",
            "outcome": "rate_limited",
            "trigger": "auto",
            "errorKind": "rate_limited",
            "executedAt": "2026-11-14T10:00:00Z",
        ]))
        XCTAssertFalse(failed.isSuccess)
        XCTAssertEqual(failed.outcomeLabel, "被限流")
        XCTAssertEqual(failed.triggerLabel, "自动")
        XCTAssertNil(failed.credits)

        let ok = try XCTUnwrap(QoderCreditRun(json: [
            "id": 8,
            "outcome": "ok",
            "credits": 100,
            "trigger": "manual",
        ]))
        XCTAssertTrue(ok.isSuccess)
        XCTAssertEqual(ok.credits, 100)
        XCTAssertEqual(ok.triggerLabel, "手动")
    }

    func testQuotaRowExposesCreditsWithoutABalance() throws {
        let row = try XCTUnwrap(WorkerQuotaRow(json: [
            "accountId": "qa_1",
            "provider": "qoder_cn",
            "status": "ok",
            "unit": "credits",
            "source": "own_pat",
            "credits": ["state": "claimable", "claimable": 1, "supported": true,
                        "today": ["campaignId": "c1", "credits": 100]],
        ]))
        XCTAssertEqual(row.sourceName, "PAT")
        let credits = try XCTUnwrap(row.credits)
        XCTAssertTrue(credits.canClaim)
        // 余额三件套必须都是 nil：Qoder 行不能伪造一个"剩余额度"。
        XCTAssertNil(row.total)
        XCTAssertNil(row.used)
        XCTAssertNil(row.remaining)
        XCTAssertNil(row.remainingText)
    }
}
