import AppKit
import Combine
import SwiftUI

private struct StatusItemLabelRootView: View {
    let displayState: StatusItemDisplayState
    let locale: Locale
    let colorScheme: ColorScheme?
    let tooltip: String

    var body: some View {
        MenuBarStatusLabel(
            remaining: displayState.remaining,
            showCodexStatus: displayState.showStatus,
            showBalance: displayState.showBalance,
            tooltip: tooltip
        )
        .environment(\.locale, locale)
        .preferredColorScheme(colorScheme)
    }
}

private final class StatusItemHostingView: NSHostingView<StatusItemLabelRootView> {
    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }
}

private final class DashboardHostingController: NSHostingController<DashboardRootView> {
    var onLayout: (() -> Void)?

    override func viewDidLayout() {
        super.viewDidLayout()
        onLayout?()
    }
}

@MainActor
final class StatusItemController: NSObject, NSPopoverDelegate {
    private let api: APIService
    private let settings: AppSettings
    private let ocx: OpenCodexController
    private let openSettings: () -> Void
    private let quitApplication: () -> Void

    private var statusItem: NSStatusItem?
    private var statusLabelView: StatusItemHostingView?
    private var popover: NSPopover?
    private var cancellables = Set<AnyCancellable>()
    private var isTearingDown = false
    private var isDashboardVisible = false
    private var displayState: StatusItemDisplayState

    init(
        api: APIService,
        settings: AppSettings,
        ocx: OpenCodexController,
        openSettings: @escaping () -> Void,
        quitApplication: @escaping () -> Void
    ) {
        self.api = api
        self.settings = settings
        self.ocx = ocx
        self.openSettings = openSettings
        self.quitApplication = quitApplication
        self.displayState = StatusItemDisplayState(
            status: api.status,
            showStatus: settings.menuBarShowCodexStatus,
            showBalance: settings.menuBarShowCodexBalance,
            languageCode: settings.languageCode,
            themeMode: settings.themeMode
        )
        super.init()
        installStatusItem()
        observeDisplayState()
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.menu = nil
        statusItem = item

        guard let button = item.button else { return }
        button.title = ""
        button.image = nil
        button.isBordered = false
        button.target = self
        button.action = #selector(handleStatusItemAction(_:))
        button.sendAction(on: [.leftMouseUp, .rightMouseUp])

        let label = StatusItemHostingView(rootView: statusLabelRootView())
        label.translatesAutoresizingMaskIntoConstraints = false
        button.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: button.leadingAnchor),
            label.trailingAnchor.constraint(equalTo: button.trailingAnchor),
            label.topAnchor.constraint(equalTo: button.topAnchor),
            label.bottomAnchor.constraint(equalTo: button.bottomAnchor)
        ])
        statusLabelView = label
        updateStatusItemLength()
    }

    private func observeDisplayState() {
        let remaining = api.$status
            .map { status in
                status?.codex?.weekly?.remaining ?? status?.codex?.five_hour?.remaining
            }
            .removeDuplicates()

        let settingsState = Publishers.CombineLatest4(
            settings.$languageCode,
            settings.$themeMode,
            settings.$menuBarShowCodexStatus,
            settings.$menuBarShowCodexBalance
        )

        Publishers.CombineLatest(remaining, settingsState)
            .map { remaining, values in
                StatusItemDisplayState(
                    remaining: remaining,
                    showStatus: values.2,
                    showBalance: values.3,
                    languageCode: values.0,
                    themeMode: values.1
                )
            }
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] state in self?.applyDisplayState(state) }
            .store(in: &cancellables)
    }

    private func applyDisplayState(_ state: StatusItemDisplayState) {
        guard !isTearingDown else { return }
        displayState = state
        statusLabelView?.rootView = statusLabelRootView()
        updateStatusItemLength()
    }

    private func statusLabelRootView() -> StatusItemLabelRootView {
        let language = AppLanguage(rawValue: displayState.languageCode) ?? .system
        let colorScheme: ColorScheme?
        switch displayState.themeMode {
        case "light": colorScheme = .light
        case "dark": colorScheme = .dark
        default: colorScheme = nil
        }
        let tooltipKey = displayState.showStatus ? "Codex" : "AICC"
        let tooltip = settings.localized(tooltipKey, languageCode: displayState.languageCode)
        return StatusItemLabelRootView(
            displayState: displayState,
            locale: language.locale,
            colorScheme: colorScheme,
            tooltip: tooltip
        )
    }

    private func updateStatusItemLength() {
        guard let statusItem, let statusLabelView else { return }
        statusLabelView.layoutSubtreeIfNeeded()
        let width = max(1, ceil(statusLabelView.fittingSize.width + 8))
        statusItem.length = width
    }

    @objc private func handleStatusItemAction(_ sender: NSStatusBarButton) {
        switch NSApp.currentEvent?.type {
        case .rightMouseDown, .rightMouseUp:
            showContextMenu()
        default:
            toggleDashboard()
        }
    }

    private func makePopover() {
        let rootView = DashboardRootView(
            api: api,
            ocx: ocx,
            settings: settings,
            openSettings: openSettings
        )
        let controller = DashboardHostingController(rootView: rootView)
        controller.onLayout = { [weak self, weak controller] in
            guard let controller else { return }
            self?.updatePopoverSize(for: controller)
        }

        let newPopover = NSPopover()
        newPopover.behavior = .transient
        newPopover.animates = true
        newPopover.delegate = self
        newPopover.contentViewController = controller
        newPopover.contentSize = NSSize(width: 350, height: 1)
        popover = newPopover
    }

    private func updatePopoverSize(for controller: DashboardHostingController) {
        guard let popover, !isTearingDown else { return }
        controller.view.layoutSubtreeIfNeeded()
        let fittingSize = controller.view.fittingSize
        let visibleHeight = NSScreen.main?.visibleFrame.height ?? 800
        let maximumHeight = min(680, max(320, visibleHeight * 0.82))
        let height = min(max(1, fittingSize.height), maximumHeight)
        let newSize = NSSize(width: 350, height: height)
        guard abs(popover.contentSize.height - newSize.height) > 1 else { return }
        popover.contentSize = newSize
    }

    private func toggleDashboard() {
        guard !isTearingDown else { return }
        if let popover, popover.isShown {
            closePopover()
            return
        }
        guard let button = statusItem?.button else { return }
        showDashboardPopover(from: button)
    }

    private func showDashboardPopover(from button: NSStatusBarButton) {
        if popover == nil { makePopover() }
        guard let popover else { return }
        if !popover.isShown {
            popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        }
        NSApp.activate(ignoringOtherApps: true)
        popover.contentViewController?.view.window?.makeKey()
    }

    private func showContextMenu() {
        guard !isTearingDown, let button = statusItem?.button else { return }
        closePopover()
        let menu = makeContextMenu()
        menu.popUp(
            positioning: nil,
            at: NSPoint(x: button.bounds.midX, y: button.bounds.minY),
            in: button
        )
    }

    private func makeContextMenu() -> NSMenu {
        let menu = NSMenu(title: "AICC")
        menu.addItem(menuItem(StatusItemMenuCommand.openDashboard.rawValue, action: #selector(openDashboardFromMenu)))
        menu.addItem(menuItem(StatusItemMenuCommand.refreshAll.rawValue, action: #selector(refreshAllFromMenu)))
        menu.addItem(.separator())
        menu.addItem(menuItem(StatusItemMenuCommand.settings.rawValue, action: #selector(openSettingsFromMenu)))
        menu.addItem(menuItem(StatusItemMenuCommand.quit.rawValue, action: #selector(quitFromMenu)))
        return menu
    }

    @objc private func openDashboardFromMenu() {
        DispatchQueue.main.async { [weak self] in self?.toggleDashboardIfNeeded() }
    }

    private func toggleDashboardIfNeeded() {
        guard !isTearingDown else { return }
        guard let button = statusItem?.button else { return }
        showDashboardPopover(from: button)
    }

    @objc private func refreshAllFromMenu() {
        Task { @MainActor [weak self] in
            guard let self, !self.isTearingDown else { return }
            await api.fetchStatus(force: true)
        }
    }

    @objc private func openSettingsFromMenu() {
        DispatchQueue.main.async { [weak self] in self?.openSettings() }
    }

    @objc private func quitFromMenu() {
        quitApplication()
    }

    private func menuItem(_ key: String, action: Selector) -> NSMenuItem {
        let item = NSMenuItem(title: settings.localized(key), action: action, keyEquivalent: "")
        item.target = self
        return item
    }

    func popoverWillShow(_ notification: Notification) {
        guard !isDashboardVisible else { return }
        isDashboardVisible = true
        ocx.panelDidAppear()
    }

    func popoverDidClose(_ notification: Notification) {
        guard let closedPopover = notification.object as? NSPopover else { return }
        if isDashboardVisible {
            isDashboardVisible = false
            ocx.panelDidDisappear()
        }
        releasePopover(closedPopover)
    }

    private func releasePopover(_ closedPopover: NSPopover) {
        guard popover === closedPopover else { return }
        closedPopover.delegate = nil
        closedPopover.contentViewController = nil
        popover = nil
    }

    private func closePopover() {
        guard let currentPopover = popover else { return }
        let wasVisible = isDashboardVisible
        currentPopover.delegate = nil
        currentPopover.performClose(nil)
        currentPopover.contentViewController = nil
        popover = nil
        if wasVisible {
            isDashboardVisible = false
            ocx.panelDidDisappear()
        }
    }

    func tearDown() {
        guard !isTearingDown else { return }
        isTearingDown = true
        cancellables.removeAll()
        closePopover()
        statusLabelView?.removeFromSuperview()
        statusLabelView = nil
        if let statusItem {
            NSStatusBar.system.removeStatusItem(statusItem)
        }
        self.statusItem = nil
    }
}
