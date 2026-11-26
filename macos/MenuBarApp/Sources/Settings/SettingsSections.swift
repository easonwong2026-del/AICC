import AppKit
import SwiftUI

// MARK: - 常规

struct GeneralSettingsSection: View {
    @ObservedObject var model: SettingsModel
    @EnvironmentObject private var settings: AppSettings

    private var selectionSummary: String {
        settings.widgetSelection.isEmpty ? "自动" : "已选 \(settings.widgetSelection.count) 项"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            SettingsPageHeader(title: "常规")

            SettingsGroup(title: "桌面小组件") {
                SettingsRow(
                    title: "显示数量",
                    help: "小尺寸最多 2 张、中尺寸最多 4 张额度卡片；一个账号的多个额度窗口合并显示。"
                ) {
                    Picker("显示数量", selection: $settings.widgetCards) {
                        Text("自动").tag(WidgetCardDisplayMode.automatic)
                        Text("1 个").tag(WidgetCardDisplayMode.one)
                        Text("2 个").tag(WidgetCardDisplayMode.two)
                        Text("4 个").tag(WidgetCardDisplayMode.four)
                        Text("全部").tag(WidgetCardDisplayMode.all)
                    }
                    .pickerStyle(.menu)
                    .labelsHidden()
                    .fixedSize()
                }

                SettingsRowDivider()

                SettingsRow(
                    title: "显示额度",
                    help: "勾选具体窗口（例如只显示 Gemini 的 7 天额度），勾选顺序即显示顺序。不勾选＝自动按 Magpie 顺序显示。"
                ) {
                    HStack(spacing: 8) {
                        Text(selectionSummary)
                            .font(ConsoleType.small)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                        Button("选择…") { model.showQuotaSelection = true }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                }
            }

            SettingsGroup(title: "关于与更新") {
                SettingsValueRow(title: "当前版本", value: model.appVersionDisplay, mono: true)

                SettingsRowDivider()

                SettingsRow(
                    title: "自动检查更新",
                    help: "启动后在后台静默检查新版本，每 24 小时最多一次，不会弹窗打扰。"
                ) {
                    Toggle("自动检查更新", isOn: $settings.autoCheckUpdates)
                        .toggleStyle(.switch)
                        .labelsHidden()
                }

                SettingsRowDivider()

                updateCheckRows

                SettingsRowDivider()

                SettingsRow(title: "项目与发布页") {
                    Button("打开") { model.openReleasePage() }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                }
            }
        }
    }

    @ViewBuilder
    private var updateCheckRows: some View {
        switch model.updateState {
        case .idle:
            SettingsRow(title: "版本更新", detail: model.updateLastCheckedText) {
                Button("检查更新") { Task { await model.checkForUpdates() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        case .checking:
            SettingsRow(title: "版本更新", detail: "正在连接更新服务器…") {
                ProgressView()
                    .controlSize(.small)
            }
        case .upToDate:
            SettingsRow(
                title: "版本更新",
                detail: model.updateLastCheckedText.map { "已是最新版本 · \($0)" } ?? "已是最新版本",
                detailTone: .ok
            ) {
                Button("重新检查") { Task { await model.checkForUpdates() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        case .updateAvailable(let info):
            let detail: String = {
                if let summary = info.releaseSummary, !summary.isEmpty {
                    return summary
                }
                if let date = info.formattedPublishedDate {
                    return "发布于 \(date)"
                }
                return "有新版本可供下载"
            }()
            SettingsRow(
                title: "发现新版本 \(info.displayVersion)",
                detail: detail,
                detailTone: .accent
            ) {
                Button("下载安装包") { model.openUpdateDownload(info) }
                    .buttonStyle(ConsolePrimaryButtonStyle())
            }
            HStack(spacing: 8) {
                Spacer(minLength: 8)
                Button("更新说明") { model.openUpdateReleaseNotes(info) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                if model.updater.ignoredVersion != info.version {
                    Button("忽略此版本") { model.ignoreUpdate(info) }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                }
            }
            .padding(.bottom, 8)
        case .incompatibleSystem(let info, let required):
            SettingsRow(
                title: "发现新版本 \(info.displayVersion)",
                detail: "需要 macOS \(required) 及以上（当前系统不满足要求）",
                detailTone: .warning
            ) {
                Button("查看说明") { model.openUpdateReleaseNotes(info) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        case .failed(let message):
            SettingsRow(title: "版本更新", detail: message, detailTone: .error) {
                Button("重试") { Task { await model.checkForUpdates() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        case .notConfigured:
            SettingsRow(title: "版本更新", detail: "未配置更新源") {
                Button("打开发布页") { model.openReleasePage() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }
}

// MARK: - 连接与设备

struct ConnectionSettingsSection: View {
    @ObservedObject var model: SettingsModel
    @EnvironmentObject private var settings: AppSettings
    @EnvironmentObject private var server: ServerManager
    @EnvironmentObject private var loginAtLaunch: LaunchAtLoginService

    private var phase: SettingsBridgePhase { model.bridgePhase }
    private var lanPhase: SettingsLANPhase { model.lanPhase }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            SettingsPageHeader(title: "连接与设备")
            magpieGroup
            bridgeGroup
        }
    }

    private var magpieGroup: some View {
        SettingsGroup(title: "Magpie") {
            SettingsRow(title: "本机端口") {
                TextField("本机端口", value: $settings.magpiePort, format: .number.grouping(.never))
                    .textFieldStyle(.roundedBorder)
                    .multilineTextAlignment(.trailing)
                    .frame(width: 88)
            }

            SettingsRowDivider()

            SettingsValueRow(title: "连接状态", value: model.connection, tone: connectionTone)

            HStack(spacing: 8) {
                Spacer(minLength: 8)
                Button("检查连接") { Task { await model.checkConnection() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                    .disabled(model.busy)
                Button("打开 Magpie") { model.openMagpie() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
            .padding(.bottom, 8)
        }
    }

    private var connectionTone: SettingsTone {
        // 正常连接不额外着色：只有"连不上"才需要一眼看到。
        model.connection.hasPrefix("无法连接") ? .error : .neutral
    }

    private var bridgeGroup: some View {
        SettingsGroup(title: "墨水屏") {
            SettingsRow(title: "启用桥接") {
                Toggle("启用墨水屏桥接", isOn: $settings.bridgeEnabled)
                    .toggleStyle(.switch)
                    .labelsHidden()
            }

            if settings.bridgeEnabled {
                SettingsRowDivider()
                SettingsValueRow(title: "运行状态", value: statusText, tone: statusTone)

                if let notice = model.tlsPhase.notice {
                    SettingsInlineNotice(
                        kind: notice.tone == .error ? .error : .warning,
                        message: notice.text
                    )
                    .padding(.bottom, 8)
                }

                if let reason = phase.failureReason {
                    SettingsInlineNotice(kind: .error, message: reason)
                        .padding(.bottom, 8)
                }

                SettingsRowDivider()

                SettingsRow(title: "配对码") {
                    HStack(spacing: 10) {
                        if model.pairingCode.isEmpty {
                            Text("未生成")
                                .font(ConsoleType.small)
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                        } else {
                            Text(model.pairingCode)
                                .font(ConsoleType.metricSmall)
                                .monospacedDigit()
                                .foregroundStyle(ConsoleTheme.text)
                                .textSelection(.enabled)
                        }
                        Button(model.pairingCode.isEmpty ? "生成" : "重新生成") {
                            Task { await model.generatePairingCode() }
                        }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(!server.isServerRunning || model.busy)
                    }
                }

                SettingsRowDivider()

                SettingsRow(
                    title: "局域网访问",
                    help: "默认只监听本机（127.0.0.1）：墨水屏与 Android 连不上，只用小组件时够用。要让墨水屏连接，必须显式打开。"
                ) {
                    Toggle("允许局域网设备连接", isOn: $settings.bridgeLANEnabled)
                        .toggleStyle(.switch)
                        .labelsHidden()
                }

                if lanPhase == .allowedWithoutListener, let warning = phase.warningText {
                    SettingsInlineNotice(kind: .warning, message: warning)
                        .padding(.bottom, 8)
                }

                SettingsRowDivider()

                SettingsRow(title: "登录时启动桥接") {
                    Toggle("登录时启动桥接", isOn: Binding(
                        get: { loginAtLaunch.isEnabled },
                        set: loginAtLaunch.setEnabled
                    ))
                    .toggleStyle(.switch)
                    .labelsHidden()
                }

                if let error = loginAtLaunch.errorMessage {
                    SettingsInlineNotice(kind: .error, message: error)
                        .padding(.bottom, 8)
                }
            }

            SettingsRowDivider()

            SettingsRow(title: "设备安全") {
                HStack(spacing: 8) {
                    Text(httpsText)
                        .font(ConsoleType.small)
                        .foregroundStyle(httpsTone == .warning ? ConsoleTheme.warning : ConsoleTheme.secondaryText)
                    Button("详情…") { model.showBridgeSecurity = true }
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                }
            }
        }
    }

    /// 证书已换但端口没重启时，运行状态不能还是绿色"一切正常"。
    /// 规则在 `SettingsTLSResetFlow.certificateStatus`，与设备安全详情共用一份。
    private var statusText: String {
        model.tlsPhase.certificateStatus?.value ?? phase.statusText
    }

    private var statusTone: SettingsTone {
        model.tlsPhase.certificateStatus?.tone ?? phase.tone
    }

    private var httpsText: String {
        guard settings.bridgeEnabled else { return "已关闭" }
        if server.lanServing { return "HTTPS 运行中" }
        if settings.bridgeLANEnabled { return "HTTPS 未启动" }
        return "仅本机"
    }

    private var httpsTone: SettingsTone {
        if server.lanServing { return .ok }
        if settings.bridgeLANEnabled { return .warning }
        return .neutral
    }
}

// MARK: - 授权与存储

struct CredentialSettingsSection: View {
    @ObservedObject var model: SettingsModel

    private var credential: SettingsCredentialView { model.credentialView }
    private var attention: SettingsCredentialAttention { model.credentialAttention }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            SettingsPageHeader(title: "授权与存储")
            authSourceGroup
            storageGroup
            managementGroup
        }
    }

    private var authSourceGroup: some View {
        SettingsGroup(title: "授权来源") {
            SettingsRow(
                title: "首选授权",
                help: "只作用于 AICC 的后台请求（签到、额度、成长查询），不决定 WorkBuddy 客户端登录哪个账号；首选那份不可用时自动兜底到另一份。Magpie 优先：直接用 Magpie 的令牌，AICC 不刷新它。自有优先：用自己的扫码授权并自行续期。"
            ) {
                Picker("首选授权", selection: Binding(
                    get: { model.worker.preferredAuthSource },
                    set: { value in Task { await model.setPreferredAuthSource(value) } }
                )) {
                    Text("Magpie 优先").tag("magpie")
                    Text("自有优先").tag("own")
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(minWidth: 200, maxWidth: 240)
            }

            if let error = model.worker.authSettingsError {
                SettingsInlineNotice(kind: .error, message: error)
                    .padding(.bottom, 8)
            }
        }
    }

    private var storageGroup: some View {
        SettingsGroup(title: "凭据存储") {
            SettingsRow(title: "当前方式") {
                HStack(spacing: 10) {
                    Text(credential.headline)
                        .font(ConsoleType.small)
                        .foregroundStyle(credential.tone == .warning ? ConsoleTheme.warning : ConsoleTheme.secondaryText)
                    switch credential.mode {
                    case .localJSON:
                        Button("改回钥匙串…") { model.confirmDisableJSON = true }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    case .keychain:
                        Button("启用本地 JSON…") { model.showCredentialRisk = true }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                            .disabled(!model.workerPhase.isRunning)
                    case .unknown:
                        // 老助手：永远点不动，不摆一个假开关。
                        // 助手只是没跑起来：保留按钮，让用户知道下一步该修什么。
                        if model.workerPhase.isRunning {
                            EmptyView()
                        } else {
                            Button("启用本地 JSON…") {}
                                .buttonStyle(ConsoleSecondaryButtonStyle())
                                .disabled(true)
                        }
                    }
                }
            }

            if let error = model.credentialStorageError {
                SettingsInlineNotice(kind: .error, message: error)
                    .padding(.bottom, 8)
            }
        }
    }

    private var managementGroup: some View {
        SettingsGroup(title: "凭据管理") {
            SettingsRow(title: "待处理") {
                HStack(spacing: 10) {
                    Text(attention.text ?? "无需处理")
                        .font(ConsoleType.small)
                        .foregroundStyle(toneColor(attention.tone))
                    Button("管理凭据…") { model.showCredentialDetail = true }
                        .buttonStyle(ConsolePrimaryButtonStyle())
                }
            }

            SettingsRowDivider()

            SettingsRow(title: "钥匙串自检", detail: secureStoreDetail) {
                Button("测试") { Task { await model.checkSecureStore() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }

    /// "未检查"是没有信息量的默认值；可用时也不报 backend 和毫秒数。
    /// 只有不可用才把原因原样带出来 —— 那时它是用户唯一能行动的信息。
    private var secureStoreDetail: String? {
        let summary = model.worker.secureStoreSummary
        if summary == "未检查" { return nil }
        return summary.hasPrefix("可用") ? "可用" : summary
    }

    private func toneColor(_ tone: SettingsTone) -> Color {
        switch tone {
        case .neutral: return ConsoleTheme.secondaryText
        case .ok: return ConsoleTheme.success
        case .warning: return ConsoleTheme.warning
        case .error: return ConsoleTheme.danger
        case .accent: return ConsoleTheme.accent
        }
    }
}

// MARK: - 高级与诊断

struct AdvancedSettingsSection: View {
    @ObservedObject var model: SettingsModel

    private var workerPhase: SettingsWorkerPhase { model.workerPhase }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            SettingsPageHeader(title: "高级与诊断")
            workerGroup
            rotationGroup
            toolsGroup
        }
    }

    private var workerGroup: some View {
        SettingsGroup(title: "账号助手") {
            SettingsValueRow(
                title: "状态",
                value: workerPhase.valueText,
                tone: workerPhase.tone,
                detail: workerPhase.tone == .neutral ? workerPhase.detail : nil
            )

            if let detail = workerPhase.detail, workerPhase.tone != .neutral {
                SettingsInlineNotice(
                    kind: workerPhase.tone == .warning ? .warning : .error,
                    message: detail
                )
                .padding(.bottom, 8)
            }

            SettingsRowDivider()

            SettingsRow(title: "自检") {
                Button("检查助手") { Task { await model.worker.refresh() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }

    /// 轮换时区只读：唯一可编辑入口是「任务中心 → 自动计划 → WorkBuddy 客户端轮换」。
    /// 两处都能改同一份计划会让人以为存了两套配置；这里只显示服务端当前值。
    private var rotationGroup: some View {
        SettingsGroup(title: "WorkBuddy 轮换") {
            SettingsRow(
                title: "账号轮换时区",
                detail: model.rotationLoaded && model.rotationError == nil
                    ? "在「任务中心 → 自动计划 → 客户端轮换」里修改。"
                    : nil,
                help: "时区跟随自动轮换计划，与任务中心里的计划是同一份数据；这里只显示，不能编辑。"
            ) {
                if !model.rotationLoaded {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("读取中")
                            .font(ConsoleType.small)
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    }
                } else if model.rotationTimezones.isEmpty {
                    Text("未读取")
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                } else {
                    Text(model.rotationTimezone)
                        .font(ConsoleType.small)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .textSelection(.enabled)
                }
            }

            if let error = model.rotationError {
                SettingsInlineNotice(kind: .error, message: error)
                    .padding(.bottom, 8)
            }
        }
    }

    private var toolsGroup: some View {
        SettingsGroup(title: "诊断") {
            SettingsRow(title: "运行日志") {
                Button("查看日志") { NSWorkspace.shared.open(AICCPaths.logsDirectory) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }

            SettingsRowDivider()

            SettingsRow(title: "退出 AICC") {
                Button("退出") { NSApp.terminate(nil) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }
}
