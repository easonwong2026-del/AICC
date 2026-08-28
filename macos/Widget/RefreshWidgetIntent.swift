import AppIntents

struct RefreshWidgetIntent: AppIntent {
    static let title: LocalizedStringResource = "Refresh Widget"
    static let description = IntentDescription("Reloads the cached AICC status.")
    static let openAppWhenRun = false

    func perform() async throws -> some IntentResult {
        .result()
    }
}
