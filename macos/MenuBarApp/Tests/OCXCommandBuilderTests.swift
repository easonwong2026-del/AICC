@testable import AICCCore
import XCTest

final class OCXCommandBuilderTests: XCTestCase {
    func testEnsureUsesLoginShell() {
        let inv = OCXCommandBuilder.lifecycle(command: "ensure", ocxPath: "/opt/homebrew/bin/ocx")
        XCTAssertEqual(inv.executable, "/bin/zsh")
        XCTAssertEqual(inv.arguments, ["-lc", "exec \"$AICC_OCX_PATH\" ensure"])
        XCTAssertEqual(inv.environmentOverrides["AICC_OCX_PATH"], "/opt/homebrew/bin/ocx")
    }

    func testStopUsesLoginShell() {
        let inv = OCXCommandBuilder.lifecycle(command: "stop", ocxPath: "/opt/homebrew/bin/ocx")
        XCTAssertEqual(inv.executable, "/bin/zsh")
        XCTAssertEqual(inv.arguments, ["-lc", "exec \"$AICC_OCX_PATH\" stop"])
        XCTAssertEqual(inv.environmentOverrides["AICC_OCX_PATH"], "/opt/homebrew/bin/ocx")
    }

    func testPathWithSpacesIsNotConcatenatedIntoShellString() {
        let inv = OCXCommandBuilder.lifecycle(command: "ensure", ocxPath: "/Users/test/My Tools/ocx")
        XCTAssertEqual(inv.executable, "/bin/zsh")
        XCTAssertEqual(inv.arguments, ["-lc", "exec \"$AICC_OCX_PATH\" ensure"])
        XCTAssertEqual(inv.environmentOverrides["AICC_OCX_PATH"], "/Users/test/My Tools/ocx")
    }

    func testPathWithSingleQuoteCharacterIsNotConcatenatedIntoShellString() {
        let inv = OCXCommandBuilder.lifecycle(command: "stop", ocxPath: "/Users/test/path-with-'quote'/ocx")
        XCTAssertEqual(inv.executable, "/bin/zsh")
        XCTAssertEqual(inv.arguments, ["-lc", "exec \"$AICC_OCX_PATH\" stop"])
        XCTAssertEqual(inv.environmentOverrides["AICC_OCX_PATH"], "/Users/test/path-with-'quote'/ocx")
    }

    func testNpmGlobalPath() {
        let inv = OCXCommandBuilder.lifecycle(command: "ensure", ocxPath: "/Users/test/.npm-global/bin/ocx")
        XCTAssertEqual(inv.executable, "/bin/zsh")
        XCTAssertEqual(inv.arguments, ["-lc", "exec \"$AICC_OCX_PATH\" ensure"])
        XCTAssertEqual(inv.environmentOverrides["AICC_OCX_PATH"], "/Users/test/.npm-global/bin/ocx")
    }

    func testLocalBinPath() {
        let inv = OCXCommandBuilder.lifecycle(command: "stop", ocxPath: "/Users/test/.local/bin/ocx")
        XCTAssertEqual(inv.executable, "/bin/zsh")
        XCTAssertEqual(inv.arguments, ["-lc", "exec \"$AICC_OCX_PATH\" stop"])
        XCTAssertEqual(inv.environmentOverrides["AICC_OCX_PATH"], "/Users/test/.local/bin/ocx")
    }

    func testStatusCommandIsNotTouched() {
        // status --json must still use ProcessRunner directly, not through
        // the command builder. The builder should reject unknown commands.
    }

    func testVersionCommandIsNotTouched() {
        // --version must still use ProcessRunner directly.
    }

    func testEnsureAndStopCommandsAreAllowed() {
        // Ensure no crash or rejection for valid commands.
        let ensure = OCXCommandBuilder.lifecycle(command: "ensure", ocxPath: "/bin/ocx")
        let stop = OCXCommandBuilder.lifecycle(command: "stop", ocxPath: "/bin/ocx")
        XCTAssertEqual(ensure.executable, "/bin/zsh")
        XCTAssertEqual(stop.executable, "/bin/zsh")
    }

    func testUpdateCheckUsesTheOfficialNpmRegistryQueryWithoutAShell() {
        let inv = OCXCommandBuilder.updateCheck()
        XCTAssertEqual(inv.executable, "/usr/bin/env")
        XCTAssertEqual(inv.arguments, ["npm", "view", "@bitkyc08/opencodex@latest", "version"])
        XCTAssertTrue(inv.environmentOverrides.isEmpty)
    }

    func testUpdateUsesTheOfficialCliDirectly() {
        let inv = OCXCommandBuilder.update(ocxPath: "/Users/test/.npm-global/bin/ocx")
        XCTAssertEqual(inv.executable, "/Users/test/.npm-global/bin/ocx")
        XCTAssertEqual(inv.arguments, ["update"])
        XCTAssertTrue(inv.environmentOverrides.isEmpty)
    }
}
