enum StatusItemMenuCommand: String, CaseIterable {
    case openDashboard = "Open AICC Dashboard"
    case refreshAll = "Refresh All Now"
    case settings = "Settings…"
    case quit = "Quit AICC"
}

enum StatusItemMouseButton: Equatable {
    case left
    case right
}

enum StatusItemClickAction: Equatable {
    case dashboard
    case contextMenu
}

enum StatusItemClickRouter {
    static func action(for button: StatusItemMouseButton) -> StatusItemClickAction {
        switch button {
        case .left: return .dashboard
        case .right: return .contextMenu
        }
    }
}
