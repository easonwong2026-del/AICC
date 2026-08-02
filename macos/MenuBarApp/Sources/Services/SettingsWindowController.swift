import AppKit
import SwiftUI

/// Fallback presenter for the SwiftUI Settings view.
///
/// `Settings` remains the app's primary scene. Menu-bar-only apps can however
/// fail to materialize that scene when the private `showSettingsWindow:`
/// action is sent while the app has no regular application window. This
/// controller hosts the exact same SettingsView and environment objects so
/// the menu-bar command still has a deterministic window to show.
@MainActor
final class SettingsWindowController: NSWindowController {
    init(
        api: APIService,
        ocx: OpenCodexController,
        settings: AppSettings,
        server: ServerManager,
        loginAtLaunch: LaunchAtLoginService
    ) {
        let rootView = SettingsView()
            .environmentObject(api)
            .environmentObject(ocx)
            .environmentObject(settings)
            .environmentObject(server)
            .environmentObject(loginAtLaunch)
            .environment(\.locale, settings.locale)
            .preferredColorScheme(settings.preferredColorScheme)

        let hostingController = NSHostingController(rootView: rootView)
        let window = NSWindow(contentViewController: hostingController)
        window.identifier = NSUserInterfaceItemIdentifier("com.aieink.dashboard.settings")
        window.title = settings.localized("Settings")
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.setContentSize(NSSize(width: 640, height: 480))
        window.minSize = NSSize(width: 560, height: 420)
        window.isReleasedWhenClosed = false
        window.center()

        super.init(window: window)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        window?.orderFrontRegardless()
    }
}
