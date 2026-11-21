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
            SettingsPageHeader(title: "常规", subtitle: "桌面小组件与更新。")

            SettingsGroup(
                title: "桌面小组件",
                footnote: "小组件独立读取 Magpie，关闭 AICC 后仍可更新。"
            ) {
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
                SettingsRow(title: "项目与更新", detail: "在 GitHub 上查看发布说明与安装包") {
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
    private var tls: BridgeTlsStatus? { model.worker.bridgeTls }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            SettingsPageHeader(title: "连接与设备", subtitle: "Magpie、墨水屏桥接与证书。")

            magpieGroup
            bridgeGroup
            securityGroup
        }
    }

    private var magpieGroup: some View {
        SettingsGroup(
            title: "Magpie",
            footnote: "端口与 Magpie 设置里的一致；改动会重启桥接并刷新小组件数据源。"
        ) {
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
            SettingsRow(
                title: "启用桥接",
                detail: settings.bridgeEnabled ? nil : "仅使用小组件时无需桥接，关闭窗口即退出 AICC。"
            ) {
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

                SettingsRow(
                    title: "配对码",
                    detail: model.pairingCode.isEmpty
                        ? "在墨水屏上输入，2 分钟内有效"
                        : "2 分钟内有效，过期需重新生成"
                ) {
                    HStack(spacing: 10) {
                        if !model.pairingCode.isEmpty {
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
                    detail: settings.bridgeLANEnabled ? lanPhase.text : nil,
                    detailTone: lanPhase.tone,
                    help: "默认只监听本机（127.0.0.1）：墨水屏与 Android 连不上，只用小组件时够用。要让墨水屏连接，必须显式打开。"
                ) {
                    Toggle("允许局域网设备连接", isOn: $settings.bridgeLANEnabled)
                        .toggleStyle(.switch)
                        .labelsHidden()
                }

                if lanPhase == .allowedWithoutListener {
                    SettingsInlineNotice(
                        kind: .warning,
                        message: "局域网已允许，但 HTTPS 监听没起来（通常是证书还没准备好）。墨水屏此时连不上，也不会退回明文 HTTP。"
                    )
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
        }
    }

    private var securityGroup: some View {
        SettingsGroup(
            title: "设备安全",
            footnote: "指纹来自根证书，叶证书续期不会改变它，因此续期不用重新配对。"
        ) {
            SettingsValueRow(title: "HTTPS 服务", value: httpsText, tone: httpsTone)
            SettingsRowDivider()

            if let tls {
                if let expiry = tls.notAfter, !expiry.isEmpty {
                    SettingsValueRow(title: "证书有效期", value: expiry)
                    SettingsRowDivider()
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text("证书指纹")
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                    Text(tls.groupedFingerprint)
                        .font(ConsoleType.monoSmall)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .textSelection(.enabled)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, 8)

                SettingsRow(title: "证书身份", detail: "重置会换掉根证书，所有已配对设备必须重新确认新指纹。") {
                    HStack(spacing: 8) {
                        Button("复制指纹") { model.copyFingerprint() }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                        Button("重置…") { model.requestTLSReset() }
                            .buttonStyle(SettingsDestructiveButtonStyle())
                            .disabled(model.tlsPhase.isBusy)
                    }
                }
            } else {
                SettingsValueRow(
                    title: "证书指纹",
                    value: "未准备",
                    detail: settings.bridgeLANEnabled
                        ? "局域网已允许，证书会在桥接启动时准备。"
                        : "打开局域网访问后会自动准备证书。"
                )
            }

            if let error = model.worker.bridgeTlsError {
                SettingsInlineNotice(kind: .error, message: error)
                    .padding(.bottom, 8)
            }

            if let notice = model.securityNotice {
                SettingsInlineNotice(kind: .info, message: notice)
                    .padding(.bottom, 8)
            }

            SettingsRowDivider()

            SettingsDangerRow(
                title: "撤销已配对设备",
                detail: "立即让所有旧配对凭证失效，每台墨水屏都要重新配对。",
                buttonTitle: "撤销…",
                disabled: model.busy
            ) {
                model.confirmRevokeDevices = true
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
        if server.lanServing { return "运行中（TLS）" }
        if settings.bridgeEnabled && settings.bridgeLANEnabled { return "未启动" }
        if settings.bridgeEnabled { return "仅本机" }
        return "已关闭"
    }

    private var httpsTone: SettingsTone {
        if server.lanServing { return .ok }
        if settings.bridgeEnabled && settings.bridgeLANEnabled { return .warning }
        return .neutral
    }
}

// MARK: - 授权与存储

struct CredentialSettingsSection: View {
    @ObservedObject var model: SettingsModel

    private var credential: SettingsCredentialView {
        SettingsCredentialView.resolve(
            model.worker.credentialStorage,
            helperRunning: model.workerPhase.isRunning
        )
    }
    private var attention: SettingsCredentialAttention { model.credentialAttention }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            SettingsPageHeader(title: "授权与存储", subtitle: "授权来源、凭据存放位置与旧凭据。")

            authSourceGroup
            storageGroup
            managementGroup
        }
    }

    private var authSourceGroup: some View {
        SettingsGroup(
            title: "授权来源",
            footnote: "只作用于 AICC 的后台请求（签到、额度、成长查询），不决定 WorkBuddy 客户端登录哪个账号。首选那份不可用时自动兜底到另一份。"
        ) {
            SettingsRow(title: "首选授权", help: "Magpie 优先：直接用 Magpie 的令牌，AICC 不刷新它。自有优先：用自己的扫码授权并自行续期。") {
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
        SettingsGroup(
            title: "凭据存储",
            footnote: credential.mode == .localJSON
                ? "凭据目录是 0700、文件是 0600。这只挡同机其他用户，挡不住同一个 macOS 用户下运行的其他程序，建议开启 FileVault。"
                : nil
        ) {
            SettingsValueRow(title: "当前方式", value: credential.headline, tone: credential.tone)

            if !credential.detail.isEmpty {
                SettingsRowDivider()
                SettingsValueRow(title: "保存位置", value: credential.detail)
            }

            SettingsRowDivider()

            SettingsRow(title: "新建授权", detail: credential.promise(helperRunning: model.workerPhase.isRunning)) {
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
                        Text("不可用")
                            .font(ConsoleType.small)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    } else {
                        Button("启用本地 JSON…") {}
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                            .disabled(true)
                    }
                }
            }

            if credential.needsConsent {
                SettingsInlineNotice(
                    kind: .info,
                    message: "尚未确认明文存储的风险：在启用之前，AICC 不会把任何新令牌写成明文文件。"
                )
                .padding(.bottom, 8)
            }

            if let error = model.credentialStorageError {
                SettingsInlineNotice(kind: .error, message: error)
                    .padding(.bottom, 8)
            }
        }
    }

    private var managementGroup: some View {
        SettingsGroup(
            title: "凭据管理",
            footnote: "迁移会先备份数据库与清单（不含令牌），原始钥匙串条目一个都不删；核验、回滚与重新关联都在详情里。"
        ) {
            SettingsValueRow(
                title: "待处理",
                value: attention.text ?? "无需处理",
                tone: attention.tone
            )

            SettingsRowDivider()

            SettingsRow(
                title: "钥匙串自检",
                detail: model.worker.secureStoreSummary,
                help: "会读写一次钥匙串探针，系统可能弹出授权框。"
            ) {
                Button("测试") { Task { await model.checkSecureStore() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }

            SettingsRowDivider()

            SettingsRow(title: "旧凭据迁移", detail: "查看逐条状态、迁移、核验与回滚") {
                Button("管理凭据…") { model.showCredentialDetail = true }
                    .buttonStyle(ConsolePrimaryButtonStyle())
            }
        }
    }
}

// MARK: - 高级与诊断

struct AdvancedSettingsSection: View {
    @ObservedObject var model: SettingsModel
    @EnvironmentObject private var settings: AppSettings

    private var workerPhase: SettingsWorkerPhase { model.workerPhase }

    var body: some View {
        VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
            SettingsPageHeader(title: "高级与诊断", subtitle: "账号助手、轮换时区、证书与日志。")

            workerGroup
            rotationGroup
            tlsGroup
            toolsGroup
            quitGroup
        }
    }

    private var workerGroup: some View {
        SettingsGroup(
            title: "账号助手",
            footnote: "助手是内嵌在 App 内的独立进程，只通过本机管道通信，不开放网络端口。"
        ) {
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

            SettingsRow(title: "自检", detail: "重新检查助手进程与协议版本") {
                Button("检查助手") { Task { await model.worker.refresh() } }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }

    private var rotationGroup: some View {
        SettingsGroup(
            title: "WorkBuddy 轮换",
            footnote: model.rotationRule.isEmpty ? nil : model.rotationRule
        ) {
            SettingsRow(
                title: "账号轮换时区",
                help: "轮换的开始与结束时间按这个时区解释。只提供固定偏移、无夏令时的时区，避免半夜算错一小时。"
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

    private var tlsGroup: some View {
        SettingsGroup(
            title: "TLS 诊断",
            footnote: "证书指纹在「连接与设备 · 设备安全」。"
        ) {
            SettingsValueRow(
                title: "证书代数",
                value: model.worker.bridgeTls?.generation.map(String.init) ?? "未准备"
            )
            SettingsRowDivider()
            SettingsValueRow(
                title: "有效期",
                value: model.worker.bridgeTls?.notAfter ?? "—"
            )
            SettingsRowDivider()
            SettingsValueRow(
                title: "最近错误",
                value: model.worker.bridgeTlsError ?? "无",
                tone: model.worker.bridgeTlsError == nil ? .neutral : .error
            )
        }
    }

    private var toolsGroup: some View {
        SettingsGroup(title: "诊断工具") {
            SettingsRow(title: "运行日志", detail: "在 Finder 中打开 AICC 的日志目录") {
                Button("查看日志") { NSWorkspace.shared.open(AICCPaths.logsDirectory) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }

    private var quitGroup: some View {
        SettingsGroup(title: "退出", footnote: "桥接关闭时，退出 AICC 会同时停止后台服务。") {
            SettingsRow(title: "退出 AICC", detail: "关闭主控制台、设置窗口与账号助手") {
                Button("退出") { NSApp.terminate(nil) }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
    }
}
