// swift-tools-version: 5.9
import PackageDescription

/// AICCCore — minimal library for testing pure model types and the process
/// runner without importing the full @main app or AppKit-dependent views.
///
/// The shell build (`build-aicc-swiftui.sh`) uses its own recursive Swift
/// source collector and is unaffected by this file.
let package = Package(
    name: "AICCCore",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "AICCCore", targets: ["AICCCore"]),
    ],
    targets: [
        .target(
            name: "AICCCore",
            path: "Sources",
            exclude: [
                "AICCApp.swift",
                "Helpers",
                "Services/BridgeCredential.swift",
                "Services/LaunchAtLoginService.swift",
                "Services/SettingsWindowCoordinator.swift",
                "Services/SingleInstanceService.swift",
                "Settings",
                "Views",
            ],
            sources: [
                "Models/QuotaModel.swift",
                "Models/WidgetPreferences.swift",
                "Models/DashboardTypography.swift",
                "Models/SettingsData.swift",
                "Models/SettingsPresentationModel.swift",
                "Models/UpdateModels.swift",
                "Services/LegacyLaunchAgentMigration.swift",
                "Services/LogFileLimiter.swift",
                "Services/ProcessRunner.swift",
                "Services/UpdateService.swift",
            ]
        ),
        .testTarget(
            name: "AICCCoreTests",
            dependencies: ["AICCCore"],
            path: "Tests",
            exclude: ["Fixtures"],
            sources: [
                "AppSettingsTests.swift",
                "ProcessRunnerTests.swift",
                "DashboardTypographyTests.swift",
                "SettingsPresentationModelTests.swift",
                "LogFileLimiterTests.swift",
                "LegacyLaunchAgentMigrationTests.swift",
                "UpdateServiceTests.swift",
            ]
        ),
    ]
)
