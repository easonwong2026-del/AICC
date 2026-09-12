import AppIntents
import WidgetKit

struct RefreshWidgetIntent: AppIntent {
    static let title: LocalizedStringResource = "Refresh Widget"
    static let description = IntentDescription("Refreshes AICC collectors and loads their latest status.")
    static let openAppWhenRun = false

    func perform() async throws -> some IntentResult {
        _ = await WidgetStatusLoader.snapshot(force: true)
        WidgetCenter.shared.reloadTimelines(ofKind: AICCWidget.kind)
        return .result()
    }
}
