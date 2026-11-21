import Foundation
import ServiceManagement

@MainActor
final class LaunchAtLoginService: ObservableObject {
    static let shared = LaunchAtLoginService()

    @Published private(set) var isEnabled = false
    @Published private(set) var errorMessage: String?

    init() {
        refresh()
    }

    func refresh() {
        isEnabled = SMAppService.mainApp.status == .enabled
    }

    func setEnabled(_ enabled: Bool) {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            errorMessage = nil
            refresh()
        } catch {
            errorMessage = error.localizedDescription
            refresh()
        }
    }
}

#if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
extension LaunchAtLoginService {
    /// 只给离屏渲染用：真实实现会去问 SMAppService。
    func applySettingsVisualState(enabled: Bool, error: String?) {
        isEnabled = enabled
        errorMessage = error
    }
}
#endif
