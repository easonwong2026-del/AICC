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
                "Services/CacheManager.swift",
                "Services/LaunchAtLoginService.swift",
                "Services/OpenCodexController.swift",
                "Services/StatusItemController.swift",
                "Services/SingleInstanceService.swift",
                "Settings",
                "Views",
            ],
            sources: [
                "Models/StatusData.swift",
                "Models/DashboardTypography.swift",
                "Models/SettingsData.swift",
                "Models/SettingsPresentationModel.swift",
                "Models/StatusItemMenuModel.swift",
                "Models/UpdateModels.swift",
                "Services/APIService.swift",
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
                "OCXStatusTests.swift",
                "OCXCommandBuilderTests.swift",
                "OCXOperationPolicyTests.swift",
                "ProcessRunnerTests.swift",
                "DashboardTypographyTests.swift",
                "SettingsPresentationModelTests.swift",
                "StatusItemMenuModelTests.swift",
                "LogFileLimiterTests.swift",
                "LegacyLaunchAgentMigrationTests.swift",
                "UpdateServiceTests.swift",
                "APIServiceTests.swift",
            ]
        ),
    ]
)
