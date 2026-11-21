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

            SettingsGroup(title: "关于") {
                SettingsRow(title: "项目与更新") {
                    Button("打开") {
                        NSWorkspace.shared.open(URL(string: "https://github.com/easonwong2026-del/AICC/releases")!)
                    }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                }
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
    private var statusText: String {
        if model.tlsPhase == .replacedButRestartFailed, phase == .loopbackOnly || phase == .lanServing {
            return "运行中 · 证书未生效"
        }
        return phase.statusText
    }

    private var statusTone: SettingsTone {
        if model.tlsPhase == .replacedButRestartFailed { return .warning }
        return phase.tone
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

    private var rotationGroup: some View {
        SettingsGroup(title: "WorkBuddy 轮换") {
            SettingsRow(
                title: "账号轮换时区",
                help: "轮换的开始与结束时间按这个时区解释；只提供固定偏移、无夏令时的时区。错过开始时间过久不补跑，每次切换前预留恢复余量。"
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
                    Picker("账号轮换时区", selection: Binding(
                        get: { model.rotationTimezone },
                        set: { zone in Task { await model.saveRotationTimezone(zone) } }
                    )) {
                        ForEach(model.rotationTimezones, id: \.self) { zone in
                            Text(zone).tag(zone)
                        }
                    }
                    .pickerStyle(.menu)
                    .labelsHidden()
                    .fixedSize()
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
