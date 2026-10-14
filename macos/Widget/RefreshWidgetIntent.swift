import AppIntents
import WidgetKit
#if AICC_HOST
import AppKit
#endif

struct RefreshWidgetIntent: AppIntent {
    static let title: LocalizedStringResource = "Refresh Widget"
    static let description = IntentDescription("Refreshes Codex quotas through local Magpie.")
    static let openAppWhenRun = false

    @Parameter(title: "Magpie Port", default: 3425)
    var magpiePort: Int

    init() { magpiePort = 3425 }
    init(port: Int) { magpiePort = port }

    func perform() async throws -> some IntentResult {
        _ = await WidgetStatusLoader.snapshot(force: true, port: magpiePort)
        WidgetCenter.shared.reloadTimelines(ofKind: AICCWidgetMetadata.kind)
        #if AICC_HOST
        await MainActor.run { (NSApp.delegate as? AICCAppDelegate)?.finishBackgroundRefresh() }
        #endif
        return .result()
    }
}
