import SwiftUI

/// 统一添加账号的请求：平台与授权方式都可以预填（详情页的"重新授权"会用到）。
struct ConsoleAddRequest: Equatable {
    var platform: ConsolePlatformKind?
    var method: ConsoleAddMethod?

    init(platform: ConsolePlatformKind? = nil, method: ConsoleAddMethod? = nil) {
        self.platform = platform
        self.method = method
    }
}

enum ConsoleAddMethod: String, CaseIterable, Identifiable {
    case browser
    case magpie
    case pat

    var id: String { rawValue }

    var title: String {
        switch self {
        case .browser: return "浏览器登录"
        case .magpie: return "从 Magpie 导入"
        case .pat: return "Personal Access Token"
        }
    }

    var detail: String {
        switch self {
        case .browser: return "在系统浏览器里完成官方授权，也可以用同一会话的二维码在手机上扫"
        case .magpie: return "复用 Magpie 里已登录的 WorkBuddy 账号，只复制访问令牌"
        case .pat: return "用 Qoder 账户页生成的长期令牌授权，适合浏览器回调走不通的情况"
        }
    }

    var symbol: String {
        switch self {
        case .browser: return "safari"
        case .magpie: return "arrow.down.doc"
        case .pat: return "key.horizontal"
        }
    }
}

/// 全页唯一的 sheet 路由状态（PRD §7.2）。
///
/// 所有弹层都从这里进：不会出现两个 sheet 抢着present，也不会互相嵌套。
enum ActiveConsoleSheet: Identifiable {
    case addAccount(ConsoleAddRequest)
    case qoderProfiles(QoderEdition)
    /// 删除 Qoder 服务账号的确认面板。做成 sheet 而不是 alert：
    /// 后果需要逐条写清楚，alert 的 message 放不下，也没法显示失败后重试。
    case deleteQoderAccount(String)

    var id: String {
        switch self {
        case .addAccount(let request):
            return "add-\(request.platform?.rawValue ?? "any")-\(request.method?.rawValue ?? "any")"
        case .qoderProfiles(let edition):
            return "profiles-\(edition.rawValue)"
        case .deleteQoderAccount(let accountId):
            return "delete-\(accountId)"
        }
    }
}

/// 统一添加账号面板：**一个**主按钮，三步选定后复用现有登录 / 导入组件。
///
/// 这里只做路由与呈现，认证协议一行都没有重写：
/// - WorkBuddy：`AddWorkbuddyAccountSheet`（浏览器 + 二维码）或 `MagpieImportSheet`
/// - Qoder：`QoderDeviceAuthSheet`（浏览器）或 `QoderPatSheet`（PAT）
struct ConsoleAddAccountSheet: View {
    @ObservedObject var store: ConsoleStore
    let onFinished: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var platform: ConsolePlatformKind?
    @State private var method: ConsoleAddMethod?

    init(store: ConsoleStore, request: ConsoleAddRequest, onFinished: @escaping () -> Void) {
        self.store = store
        self.onFinished = onFinished
        _platform = State(initialValue: request.platform)
        _method = State(initialValue: request.method)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            Group {
                if let platform, let method {
                    component(for: platform, method: method)
                } else if let platform {
                    methodStep(platform)
                } else {
                    platformStep
                }
            }
            Divider().overlay(ConsoleTheme.border)
            footer
        }
        .frame(width: 480)
        .background(ConsoleTheme.canvas)
    }

    // MARK: 头部

    private var header: some View {
        HStack(spacing: 9) {
            Image(systemName: "person.badge.plus")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 28, height: 28)
                .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 1) {
                Text("添加账号")
                    .font(ConsoleType.sectionTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text(breadcrumb)
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
        }
        .padding(14)
    }

    private var breadcrumb: String {
        var parts: [String] = []
        if let platform { parts.append(platform.title) }
        if let method { parts.append(method.title) }
        if parts.isEmpty { return "第一步：选择平台" }
        if method == nil { return "第二步：选择授权方式" }
        return parts.joined(separator: " · ")
    }

    // MARK: 第一步：平台

    private var platformStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("要添加哪个平台的账号？")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
            ForEach(ConsolePlatformKind.allCases) { kind in
                Button {
                    platform = kind
                    method = nil
                } label: {
                    HStack(spacing: 10) {
                        Image(systemName: kind.symbol)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.accent)
                            .frame(width: 26, height: 26)
                            .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 7))
                        VStack(alignment: .leading, spacing: 1) {
                            Text(kind.title)
                                .font(ConsoleType.rowTitle)
                                .foregroundStyle(ConsoleTheme.text)
                            Text(kind.subtitle)
                                .font(ConsoleType.caption)
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                        }
                        Spacer(minLength: 6)
                        Image(systemName: "chevron.right")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 9))
                    .overlay(
                        RoundedRectangle(cornerRadius: 9).strokeBorder(ConsoleTheme.border, lineWidth: 1)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(14)
    }

    // MARK: 第二步：授权方式

    private func methodStep(_ platform: ConsolePlatformKind) -> some View {
        let methods = availableMethods(for: platform)
        return VStack(alignment: .leading, spacing: 8) {
            Text("用哪种方式授权？")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
            ForEach(methods) { option in
                Button {
                    method = option
                } label: {
                    HStack(spacing: 10) {
                        Image(systemName: option.symbol)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.accent)
                            .frame(width: 26, height: 26)
                            .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 7))
                        VStack(alignment: .leading, spacing: 1) {
                            Text(option.title)
                                .font(ConsoleType.rowTitle)
                                .foregroundStyle(ConsoleTheme.text)
                            Text(option.detail)
                                .font(ConsoleType.caption)
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                                .fixedSize(horizontal: false, vertical: true)
                                .multilineTextAlignment(.leading)
                        }
                        Spacer(minLength: 6)
                        Image(systemName: "chevron.right")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 9))
                    .overlay(
                        RoundedRectangle(cornerRadius: 9).strokeBorder(ConsoleTheme.border, lineWidth: 1)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            if platform == .qoderCN || platform == .qoderGlobal {
                Text("Magpie 里没有 Qoder 的服务账号可导入，所以这里不提供该入口。")
                    .font(ConsoleType.caption)
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
        }
        .padding(14)
    }

    /// 只列真实可用的方式：Qoder 没有 Magpie 导入，WorkBuddy 没有 PAT。
    private func availableMethods(for platform: ConsolePlatformKind) -> [ConsoleAddMethod] {
        switch platform {
        case .workbuddyCN, .workbuddyAI: return [.browser, .magpie]
        case .qoderCN, .qoderGlobal: return [.browser, .pat]
        }
    }

    // MARK: 第三步：复用现有组件

    @ViewBuilder
    private func component(for platform: ConsolePlatformKind, method: ConsoleAddMethod) -> some View {
        switch (platform, method) {
        case (.workbuddyCN, .browser):
            AddWorkbuddyAccountSheet(store: store, initialVariant: .cn, onFinished: onFinished)
        case (.workbuddyAI, .browser):
            AddWorkbuddyAccountSheet(store: store, initialVariant: .ai, onFinished: onFinished)
        case (.workbuddyCN, .magpie), (.workbuddyAI, .magpie):
            MagpieImportSheet(store: store)
        case (.qoderCN, .browser):
            QoderDeviceAuthSheet(store: store, initialEdition: .cn, onFinished: onFinished)
        case (.qoderGlobal, .browser):
            QoderDeviceAuthSheet(store: store, initialEdition: .global, onFinished: onFinished)
        case (.qoderCN, .pat):
            QoderPatSheet(store: store, initialEdition: .cn, onFinished: onFinished)
        case (.qoderGlobal, .pat):
            QoderPatSheet(store: store, initialEdition: .global, onFinished: onFinished)
        default:
            ConsoleEmptyState(
                symbol: "questionmark.circle",
                title: "这个组合没有对应流程",
                message: "请返回上一步重新选择平台或授权方式。"
            )
        }
    }

    // MARK: 底部

    private var footer: some View {
        HStack(spacing: 8) {
            if platform != nil && method == nil {
                Button("上一步") {
                    platform = nil
                    method = nil
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
            } else if method != nil {
                Button("重新选择") {
                    method = nil
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
            }
            Spacer(minLength: 8)
            Button("关闭") { dismiss() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
        }
        .padding(12)
    }
}
