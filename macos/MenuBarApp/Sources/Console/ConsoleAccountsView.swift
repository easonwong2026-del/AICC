import AppKit
import SwiftUI

/// 「AI 账号」= 三个平台：WorkBuddy、Qoder CN、Qoder International。
///
/// WorkBuddy 的账号来自 `accounts` 表；两个 Qoder 版本来自 `qoder_profiles`。
///
/// ⚠️ 本阶段 Qoder 是**只读管理**：识别 / 扫描 / 导入 / 认证检查 / 保存快照。
/// 账号切换在完整隔离（`main.sqlite` 的账号数据、Cookies、Local Storage）落地前
/// 不开放，界面上不提供任何写入 live 的入口。
struct ConsoleAccountsView: View {
    @ObservedObject var store: ConsoleStore

    @State private var filter: ConsolePlatformFilter = .all
    @State private var query = ""
    @State private var showAdd = false
    @State private var showMagpie = false
    @State private var showQoderSheet = false
    @State private var qoderSheetEdition: QoderEdition = .global
    @State private var renaming: QoderProfile?
    @State private var renameText = ""
    @State private var removing: QoderProfile?
    @State private var confirmPurge = false
    @State private var showRecoverConfirm = false

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    /// 国内版与**国际版**是两类账号：分组显示，不混在一个 "WorkBuddy" 里。
    private func visibleWorkbuddy(_ international: Bool) -> [ConsoleAccount] {
        guard international ? filter.showsWorkbuddyAI : filter.showsWorkbuddyCN else { return [] }
        return store.accounts.filter { account in
            account.isWorkbuddy(international: international)
                && (trimmedQuery.isEmpty || account.searchText.contains(trimmedQuery))
        }
    }

    private func visibleProfiles(_ edition: QoderEdition) -> [QoderProfile] {
        guard filter.showsQoder else { return [] }
        guard filter.edition == nil || filter.edition == edition else { return [] }
        return store.qoderProfiles.filter { profile in
            profile.edition == edition.rawValue
                && (trimmedQuery.isEmpty || profile.searchText.contains(trimmedQuery))
        }
    }

    /// Qoder 平台服务账号（Magpie 导入 / 网页授权）：与本机快照分开显示。
    private func visibleServiceAccounts(_ edition: QoderEdition) -> [QoderServiceAccount] {
        guard filter.showsQoder else { return [] }
        guard filter.edition == nil || filter.edition == edition else { return [] }
        return store.qoderServiceAccounts.filter { account in
            account.edition == edition.rawValue
                && (trimmedQuery.isEmpty || account.searchText.contains(trimmedQuery))
        }
    }

    private var totalVisible: Int {
        visibleWorkbuddy(false).count
            + visibleWorkbuddy(true).count
            + QoderEdition.allCases.reduce(0) {
                $0 + visibleProfiles($1).count + visibleServiceAccounts($1).count
            }
    }

    private var hasAnything: Bool {
        !store.accounts.isEmpty
            || !store.qoderProfiles.isEmpty
            || !store.qoderServiceAccounts.isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            toolbar
            readOnlyStageNote
            if !store.qoderPendingSwitches.isEmpty { pendingSwitchBanner }
            if let error = store.accountsError { banner(error, tone: .error) }
            if let error = store.qoderError { banner(error, tone: .error) }
            if let notice = store.qoderNotice { banner(notice, tone: .info) }
            content
        }
        .task {
            // QA 钩子：`open --args -AICCOpenAddAccount 1` 直接拉起扫码面板。
            if UserDefaults.standard.bool(forKey: "AICCOpenAddAccount")
                || ProcessInfo.processInfo.environment["AICC_OPEN_ADD_ACCOUNT"] == "1" {
                showAdd = true
            }
            // 同理：`open --args -AICCOpenMagpieImport 1` 直接拉起 Magpie 导入面板。
            if UserDefaults.standard.bool(forKey: "AICCOpenMagpieImport")
                || ProcessInfo.processInfo.environment["AICC_OPEN_MAGPIE_IMPORT"] == "1" {
                showMagpie = true
            }
            if store.qoderProfiles.isEmpty {
                await store.scanQoderProfiles()
            }
        }
        .sheet(isPresented: $showAdd) {
            AddWorkbuddyAccountSheet(store: store) {
                Task { await store.refreshAccounts() }
            }
        }
        .sheet(isPresented: $showMagpie) {
            MagpieImportSheet(store: store)
        }
        .sheet(isPresented: $showQoderSheet) {
            QoderProfileSheet(store: store, initialEdition: qoderSheetEdition)
        }
        .alert("重命名 Profile", isPresented: Binding(
            get: { renaming != nil },
            set: { if !$0 { renaming = nil } }
        )) {
            TextField("显示名称", text: $renameText)
            Button("取消", role: .cancel) { renaming = nil }
            Button("保存") {
                if let profile = renaming {
                    let name = renameText
                    Task { await store.qoderRename(profile, to: name) }
                }
                renaming = nil
            }
        } message: {
            Text("只改 AICC 里显示的名称，不会改动 Qoder 客户端里的任何数据。")
        }
        .alert("恢复未完成的切换？", isPresented: $showRecoverConfirm) {
            Button("取消", role: .cancel) {}
            Button("确认恢复") { Task { await store.qoderRecover() } }
        } message: {
            Text("AICC 会先确认对应的 Qoder 客户端已经完全退出；"
                + "无法确认（进程状态未知）时会拒绝恢复，并保持 live 认证原样。")
        }
        .confirmationDialog(
            removing.map { "移除 \($0.displayName)？" } ?? "移除 Profile",
            isPresented: Binding(
                get: { removing != nil },
                set: { if !$0 { removing = nil; confirmPurge = false } }
            ),
            titleVisibility: .visible
        ) {
            if let profile = removing {
                Button("从 AICC 移除（保留磁盘数据）") {
                    Task { await store.qoderRemove(profile, purge: false) }
                    removing = nil
                }
                Button("永久删除 Profile 与用户数据目录", role: .destructive) {
                    confirmPurge = true
                    Task { await store.qoderRemove(profile, purge: true) }
                    removing = nil
                }
                Button("取消", role: .cancel) { removing = nil }
            }
        } message: {
            if let profile = removing {
                Text(
                    "「从 AICC 移除」只删登记，Qoder 客户端里的登录状态与本地数据都保留。\n"
                    + "「永久删除」会删除 AICC 保存的账号快照 \(profile.snapshotDir)，"
                    + "这份快照里的登录态随之消失，无法撤销（客户端数据目录不会被动）。"
                )
            }
        }
    }

    private var toolbar: some View {
        HStack(spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                TextField("搜索昵称、UID 或邮箱", text: $query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12))
                    .foregroundStyle(ConsoleTheme.text)
                if !query.isEmpty {
                    Button {
                        query = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 11))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 9)
            .frame(width: 200, height: 26)
            .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 7))
            .overlay(
                RoundedRectangle(cornerRadius: 7).strokeBorder(ConsoleTheme.border, lineWidth: 1)
            )

            Picker("平台", selection: $filter) {
                ForEach(ConsolePlatformFilter.accountFilters) { option in
                    Text(option.title).tag(option)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)
            .frame(width: 420)

            Spacer(minLength: 8)

            Text("\(totalVisible) 个账号")
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.secondaryText)

            Button("扫描 Qoder 配置") {
                Task { await store.scanQoderProfiles() }
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .disabled(store.qoderActing != nil)
            .help("把本机已有的 Qoder CN / International 配置登记成 Profile，不移动、不覆盖")

            Button("从 Magpie 导入") { showMagpie = true }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .help("复用 Magpie 已登录的 WorkBuddy 账号（只复制访问令牌）")

            // 主入口现在是浏览器登录（扫码仍然可用），按钮文案别再只写"扫码"。
            Button("添加账号") { showAdd = true }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .help("在浏览器中登录或扫码，国内版 / 国际版都在面板里选")
        }
    }

    /// 本阶段的定位说明，避免用户以为这里能切账号。
    private var readOnlyStageNote: some View {
        HStack(spacing: 8) {
            Image(systemName: "lock.shield")
                .font(.system(size: 11))
            Text("Qoder 当前为只读管理：版本识别、扫描、导入、认证检查、保存快照。"
                + "账号切换在完整隔离方案落地前不开放。")
                .font(.system(size: 11))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
        }
        .foregroundStyle(ConsoleTheme.secondaryText)
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 9))
    }

    /// 只报告、不自动恢复：恢复需要用户显式确认。
    private var pendingSwitchBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: "clock.arrow.circlepath")
                .font(.system(size: 11))
            Text("检测到 \(store.qoderPendingSwitches.count) 条未完成的切换记录。"
                + "AICC 不会自动恢复；请先完全退出 Qoder 客户端，再手动恢复。")
                .font(.system(size: 11))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            Button("恢复…") { showRecoverConfirm = true }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(store.qoderActing != nil)
        }
        .foregroundStyle(ConsoleTheme.warning)
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(ConsoleTheme.warning.opacity(0.10), in: RoundedRectangle(cornerRadius: 9))
    }

    private enum BannerTone { case error, info }

    private func banner(_ message: String, tone: BannerTone) -> some View {
        let color = tone == .error ? ConsoleTheme.warning : ConsoleTheme.secondaryText
        return HStack(spacing: 8) {
            Image(systemName: tone == .error ? "exclamationmark.triangle" : "info.circle")
                .font(.system(size: 11))
            Text(message)
                .font(.system(size: 11))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            if tone == .error {
                Button("重试") { Task { await store.refreshQoder() } }
                    .buttonStyle(.borderless)
                    .font(.system(size: 11, weight: .medium))
            }
        }
        .foregroundStyle(color)
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(color.opacity(0.10), in: RoundedRectangle(cornerRadius: 9))
    }

    @ViewBuilder
    private var content: some View {
        if !hasAnything {
            ConsoleCard {
                ConsoleEmptyState(
                    symbol: "person.crop.rectangle.stack",
                    title: "还没有账号",
                    message: "WorkBuddy 可以扫码或从 Magpie 导入；Qoder 支持 CN 与 International 两个版本，"
                        + "每个账号一份独立 Profile。",
                    actionTitle: "扫描本机 Qoder 配置",
                    action: { Task { await store.scanQoderProfiles() } }
                )
            }
        } else if totalVisible == 0 {
            ConsoleCard {
                ConsoleEmptyState(
                    symbol: "line.3.horizontal.decrease.circle",
                    title: "没有匹配的账号",
                    message: "换个关键词，或把平台筛选切回「全部」。"
                )
            }
        } else {
            VStack(alignment: .leading, spacing: 22) {
                if filter.showsWorkbuddyCN, !visibleWorkbuddy(false).isEmpty {
                    workbuddySection(international: false)
                }
                if filter.showsWorkbuddyAI, !visibleWorkbuddy(true).isEmpty {
                    workbuddySection(international: true)
                }
                if filter.showsQoder {
                    ForEach(QoderEdition.allCases) { edition in
                        if filter.edition == nil || filter.edition == edition {
                            if !visibleServiceAccounts(edition).isEmpty {
                                qoderServiceSection(edition)
                            }
                            qoderSection(edition)
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func workbuddySection(international: Bool) -> some View {
        let accounts = visibleWorkbuddy(international)
        VStack(alignment: .leading, spacing: 10) {
            ConsoleSectionTitle(
                title: international ? "WorkBuddy AI（国际版）" : "WorkBuddy（国内版）",
                detail: "\(accounts.count) 个账号"
            )
            if accounts.isEmpty {
                ConsoleCard {
                    Text(international
                        ? "还没有 WorkBuddy AI 账号。点「添加账号」→ 选 WorkBuddy AI → 在浏览器中登录。"
                        : "还没有 WorkBuddy 账号。点「添加账号」，或从 Magpie 导入已登录的账号。")
                        .font(.system(size: 11))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                }
            } else {
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 300), spacing: ConsoleTheme.cardSpacing)],
                    spacing: ConsoleTheme.cardSpacing
                ) {
                    ForEach(accounts) { account in
                        ConsoleAccountCard(account: account, onRelogin: { showAdd = true })
                    }
                }
            }
        }
    }

    /// 平台服务账号：来自 Magpie 导入或浏览器授权，**不需要本机装 Qoder 客户端**。
    ///
    /// 和下面「本机 Profile」是两条独立的线：这里显示的是服务端账号身份，
    /// 那里显示的是本机客户端保存的认证快照。
    @ViewBuilder
    private func qoderServiceSection(_ edition: QoderEdition) -> some View {
        let accounts = visibleServiceAccounts(edition)
        VStack(alignment: .leading, spacing: 10) {
            ConsoleSectionTitle(
                title: "\(edition.displayName) 账号",
                detail: "\(accounts.count) 个服务账号"
            )
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 300), spacing: ConsoleTheme.cardSpacing)],
                spacing: ConsoleTheme.cardSpacing
            ) {
                ForEach(accounts) { account in
                    qoderServiceCard(account)
                }
            }
        }
    }

    private func qoderServiceCard(_ account: QoderServiceAccount) -> some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Text(account.displayName)
                        .font(.system(size: 12, weight: .semibold))
                        .lineLimit(1)
                    Spacer(minLength: 8)
                    Text(account.statusLabel)
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(qoderStatusColor(account.statusTone))
                }
                HStack(spacing: 6) {
                    Text(account.editionName)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    Text("·")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    Text("UID \(account.maskedUid)")
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                HStack(spacing: 6) {
                    ForEach(account.sourceLabels, id: \.self) { label in
                        Text(label)
                            .font(.system(size: 10, weight: .medium))
                            .padding(.horizontal, 7)
                            .padding(.vertical, 3)
                            .background(
                                ConsoleTheme.tertiaryText.opacity(0.12),
                                in: Capsule()
                            )
                            .foregroundStyle(ConsoleTheme.secondaryText)
                    }
                    if account.authorizations.isEmpty {
                        Text("还没有授权来源")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                    Spacer(minLength: 0)
                }
            }
        }
    }

    private func qoderStatusColor(_ tone: QoderServiceStatusTone) -> Color {
        switch tone {
        case .ok: return ConsoleTheme.secondaryText
        case .warn: return ConsoleTheme.warning
        case .muted: return ConsoleTheme.tertiaryText
        }
    }

    @ViewBuilder
    private func qoderSection(_ edition: QoderEdition) -> some View {
        let profiles = visibleProfiles(edition)
        let info = store.qoderEditions.first { $0.edition == edition.rawValue }
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                ConsoleSectionTitle(
                    title: edition.displayName,
                    detail: "\(profiles.count) 个 Profile"
                )
                if let info {
                    Text(info.installSummary)
                        .font(.system(size: 10))
                        .foregroundStyle(info.installed ? ConsoleTheme.tertiaryText : ConsoleTheme.warning)
                }
                if let info {
                    Text(info.parallelLabel)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
                if let hint = info?.currentAccountHint {
                    Text("客户端当前：\(hint)")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 8)
                Button {
                    qoderSheetEdition = edition
                    showQoderSheet = true
                } label: {
                    Label("添加账号", systemImage: "plus")
                        .font(.system(size: 11, weight: .medium))
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .help("新建一个独立 Profile，或用已有目录导入")
            }
            if let info, !info.switchEnabled, let reason = info.switchBlockedReason {
                HStack(spacing: 6) {
                    Image(systemName: "lock.shield")
                        .font(.system(size: 10))
                    Text("账号切换未开放：\(reason)")
                        .font(.system(size: 10))
                        .fixedSize(horizontal: false, vertical: true)
                        .lineLimit(3)
                }
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 8))
            }
            if profiles.isEmpty {
                ConsoleCard {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("还没有 \(edition.displayName) 的 Profile。")
                            .font(.system(size: 11))
                            .foregroundStyle(ConsoleTheme.secondaryText)
                        if let info, !info.installed {
                            Text("本机没有检测到 \(edition.displayName) 客户端（Bundle \(info.bundleId)），"
                                + "可以先建 Profile，装好客户端后再登录。")
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            } else {
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 320), spacing: ConsoleTheme.cardSpacing)],
                    spacing: ConsoleTheme.cardSpacing
                ) {
                    ForEach(profiles) { profile in
                        QoderProfileCard(
                            profile: profile,
                            busy: store.qoderActing == profile.id,
                            onOpen: { Task { await store.qoderOpen(profile) } },
                            onClose: { Task { await store.qoderClose(profile) } },
                            onActivate: { Task { await store.qoderActivate(profile) } },
                            onInspect: { Task { await store.qoderInspect(profile) } },
                            onCapture: { Task { await store.qoderCapture(profile) } },
                            onRename: {
                                renameText = profile.displayName
                                renaming = profile
                            },
                            onRemove: { removing = profile }
                        )
                    }
                }
            }
        }
    }
}

// MARK: - Qoder Profile 卡片

private struct QoderProfileCard: View {
    let profile: QoderProfile
    let busy: Bool
    let onOpen: () -> Void
    let onClose: () -> Void
    let onActivate: () -> Void
    let onInspect: () -> Void
    let onCapture: () -> Void
    let onRename: () -> Void
    let onRemove: () -> Void

    var body: some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: profile.symbolName)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.accent)
                        .frame(width: 28, height: 28)
                        .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))

                    VStack(alignment: .leading, spacing: 3) {
                        Text(profile.displayName)
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                        HStack(spacing: 5) {
                            Text(profile.editionName)
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.secondaryText)
                            Text(profile.managed ? "AICC 托管" : "仅登记")
                                .font(.system(size: 9, weight: .medium))
                                .foregroundStyle(ConsoleTheme.tertiaryText)
                                .padding(.horizontal, 4)
                                .padding(.vertical, 1)
                                .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 3))
                        }
                    }

                    Spacer(minLength: 6)

                    VStack(alignment: .trailing, spacing: 5) {
                        ConsoleBadge(
                            text: profile.authLabel,
                            tint: ConsoleTheme.tone(profile.authTone)
                        )
                        ConsoleBadge(
                            text: profile.runtimeLabel,
                            tint: ConsoleTheme.tone(profile.runtimeTone)
                        )
                    }
                }

                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Text(profile.identityLabel)
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                        Text(profile.identityText ?? "未识别（未登录或客户端未写入身份）")
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(ConsoleTheme.secondaryText)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                    }
                    HStack(spacing: 6) {
                        Text("快照")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                        Text(profile.snapshotDir)
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help("快照目录：\(profile.snapshotDir)\n客户端数据目录：\(profile.liveDir)")
                    }
                }

                Divider().overlay(ConsoleTheme.border)

                HStack(spacing: 14) {
                    labelValue("Token 指纹", profile.fingerprintShort)
                    labelValue("到期", shortDate(profile.expiresAt))
                    labelValue("最后打开", shortTimestamp(profile.lastOpenedAt))
                    Spacer(minLength: 0)
                }

                if profile.runtime.isUnknown {
                    Text("有进程存活但无法确认归属（读不到命令行或缺少 AICC 标记），AICC 不会对它发信号。")
                        .font(.system(size: 9))
                        .foregroundStyle(ConsoleTheme.warning)
                        .fixedSize(horizontal: false, vertical: true)
                } else if profile.snapshotIncomplete {
                    Text("上一次保存快照没有完成，快照可能只有半套文件。"
                        + "点「保存当前登录」会先恢复到上一份完整快照，再重新保存。")
                        .font(.system(size: 9))
                        .foregroundStyle(ConsoleTheme.warning)
                        .fixedSize(horizontal: false, vertical: true)
                } else if !profile.snapshotReady {
                    Text("还没有认证快照：先在 Qoder 客户端里登录这个账号，再回来点「保存当前登录」。")
                        .font(.system(size: 9))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .fixedSize(horizontal: false, vertical: true)
                } else if profile.active {
                    Text("客户端当前登录的就是这个账号。")
                        .font(.system(size: 9))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }

                HStack(spacing: 8) {
                    if profile.runtime.isRunning {
                        Button("关闭", action: onClose)
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                        Button(
                            profile.switchEnabled
                                ? (profile.active ? "已在此账号" : "切换并打开")
                                : "切换未开放",
                            action: onActivate
                        )
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .disabled(profile.active || !profile.switchEnabled)
                        .help(profile.switchBlockedReason ?? "切换到该账号")
                    } else if profile.runtime.isUnknown {
                        Button("关闭（需确认归属）", action: onClose)
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                            .help("进程还活着，但无法确认是不是 AICC 启动的；关闭时会拒绝发信号")
                    } else {
                        Button(profile.primaryActionTitle, action: onOpen)
                            .buttonStyle(ConsolePrimaryButtonStyle())
                            .disabled(!profile.switchEnabled)
                            .help(profile.switchBlockedReason ?? "打开/切换这个账号")
                    }
                    Button("保存当前登录", action: onCapture)
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                        .help("把客户端此刻登录的账号存进这个 Profile 的快照")
                    Button("检查授权", action: onInspect)
                        .buttonStyle(ConsoleSecondaryButtonStyle())
                    Spacer(minLength: 0)
                    Menu {
                        Button("重命名", action: onRename)
                        Button("从 AICC 移除…", action: onRemove)
                    } label: {
                        Image(systemName: "ellipsis")
                            .font(.system(size: 11, weight: .semibold))
                    }
                    .menuStyle(.borderlessButton)
                    .frame(width: 28)
                    .disabled(profile.runtime.isRunning)
                    .help(profile.runtime.isRunning ? "运行中的 Profile 不能删除，请先关闭" : "更多操作")
                }
                .disabled(busy)
                .opacity(busy ? 0.55 : 1)
            }
        }
        .contextMenu {
            if profile.runtime.isRunning {
                Button("关闭", action: onClose)
                if profile.switchEnabled {
                    Button("切换", action: onActivate)
                }
            } else if profile.switchEnabled {
                Button("打开", action: onOpen)
            }
            Button("检查授权", action: onInspect)
            Button("保存当前登录", action: onCapture)
            Button("重命名", action: onRename)
            Divider()
            Button("移除…", action: onRemove)
                .disabled(profile.runtime.isRunning)
        }
    }

    private func labelValue(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Text(value)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.text)
                .lineLimit(1)
        }
    }

    private func shortDate(_ value: String?) -> String {
        guard let value, value.count >= 10 else { return "—" }
        return String(value.prefix(10))
    }

    private func shortTimestamp(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "—" }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime]
        guard let date = parser.date(from: value) else { return String(value.prefix(10)) }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }
}

// MARK: - 添加 / 导入 Profile

struct QoderProfileSheet: View {
    @ObservedObject var store: ConsoleStore
    let initialEdition: QoderEdition

    @Environment(\.dismiss) private var dismiss
    @State private var edition: QoderEdition = .global
    @State private var mode: Mode = .create
    @State private var name = ""
    @State private var path = ""
    @State private var showScanner = false

    private enum Mode: String, CaseIterable, Identifiable {
        case create
        case importExisting

        var id: String { rawValue }
        var title: String {
            switch self {
            case .create: return "新建 Profile"
            case .importExisting: return "导入已有目录"
            }
        }
    }

    private var info: QoderEditionInfo? {
        store.qoderEditions.first { $0.edition == edition.rawValue }
    }

    private var canSubmit: Bool {
        switch mode {
        case .create: return true
        case .importExisting:
            return path.hasPrefix("/") && !path.trimmingCharacters(in: .whitespaces).isEmpty
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("添加 Qoder 账号")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(ConsoleTheme.text)

            Picker("版本", selection: $edition) {
                ForEach(QoderEdition.allCases) { item in
                    Text(item.displayName).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            if let info {
                HStack(spacing: 6) {
                    Image(systemName: info.installed ? "checkmark.circle" : "exclamationmark.triangle")
                        .font(.system(size: 10))
                    Text("\(info.bundleId) · \(info.installSummary)")
                        .font(.system(size: 10))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .foregroundStyle(info.installed ? ConsoleTheme.tertiaryText : ConsoleTheme.warning)
            }

            Picker("方式", selection: $mode) {
                ForEach(Mode.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            VStack(alignment: .leading, spacing: 6) {
                Text("显示名称")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                TextField(edition.defaultProfileName, text: $name)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12))
            }

            if mode == .importExisting {
                VStack(alignment: .leading, spacing: 6) {
                    Text("已有账号快照目录（里面要有 auth.v1.dat）")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    HStack(spacing: 8) {
                        TextField("选择包含 auth.v1.dat 的目录", text: $path)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: 11, design: .monospaced))
                        Button("选择…") { pickDirectory() }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                    Text("只登记引用：不移动、不覆盖任何文件，也不要求重新登录。")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                }
            } else {
                Text("会创建一个空的账号快照。创建后请手动打开 \(edition.displayName) 客户端登录，"
                    + "登录完成再回 AICC 点「保存当前登录」。AICC 不接触账号密码。\n"
                    + "注意：当前阶段不提供账号切换，AICC 不会改写客户端正在使用的认证文件。")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Spacer(minLength: 0)

            HStack {
                Button("取消") { dismiss() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                Spacer()
                Button(mode == .create ? "创建" : "导入") {
                    let displayName = name.trimmingCharacters(in: .whitespacesAndNewlines)
                    let finalName = displayName.isEmpty ? edition.defaultProfileName : displayName
                    let importPath = path.trimmingCharacters(in: .whitespacesAndNewlines)
                    let importMode = mode == .importExisting
                    let targetEdition = edition
                    dismiss()
                    Task {
                        if importMode {
                            await store.importQoderProfile(
                                edition: targetEdition,
                                snapshotDir: importPath,
                                displayName: finalName
                            )
                        } else {
                            await store.addQoderProfile(
                                edition: targetEdition,
                                displayName: finalName
                            )
                        }
                    }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(!canSubmit)
            }
        }
        .padding(20)
        .frame(width: 460)
        .background(ConsoleTheme.canvas)
        .onAppear {
            edition = initialEdition
            name = initialEdition.defaultProfileName
        }
    }

    private func pickDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = FileManager.default.homeDirectoryForCurrentUser
        if panel.runModal() == .OK, let url = panel.url {
            path = url.path
            if name.trimmingCharacters(in: .whitespaces).isEmpty {
                name = url.lastPathComponent
            }
        }
    }
}

// MARK: - WorkBuddy 卡片（沿用原有版式）

private struct ConsoleAccountCard: View {
    let account: ConsoleAccount
    let onRelogin: () -> Void

    var body: some View {
        ConsoleCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: account.symbolName)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(ConsoleTheme.accent)
                        .frame(width: 28, height: 28)
                        .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))

                    VStack(alignment: .leading, spacing: 3) {
                        Text(account.nickname)
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(ConsoleTheme.text)
                            .lineLimit(1)
                        HStack(spacing: 5) {
                            Text(account.platformName)
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.secondaryText)
                            if account.isWorkbuddyAI {
                                Text("国际版")
                                    .font(.system(size: 9, weight: .medium))
                                    .foregroundStyle(ConsoleTheme.accent)
                                    .padding(.horizontal, 4)
                                    .padding(.vertical, 1)
                                    .background(
                                        ConsoleTheme.accentSoft,
                                        in: RoundedRectangle(cornerRadius: 3)
                                    )
                            }
                            if let source = account.sourceLabel {
                                Text(source)
                                    .font(.system(size: 9, weight: .medium))
                                    .foregroundStyle(ConsoleTheme.tertiaryText)
                                    .padding(.horizontal, 4)
                                    .padding(.vertical, 1)
                                    .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 3))
                            }
                        }
                    }

                    Spacer(minLength: 6)

                    ConsoleBadge(
                        text: account.displayAuthLabel,
                        tint: ConsoleTheme.tone(account.displayAuthTone)
                    )
                }

                HStack(spacing: 6) {
                    Text("UID")
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.tertiaryText)
                    Text(account.uid)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }

                if !account.authorizations.isEmpty {
                    VStack(alignment: .leading, spacing: 5) {
                        HStack(spacing: 6) {
                            AuthorizationChip(kindLabel: "自有", authorization: account.ownAuthorization)
                            AuthorizationChip(kindLabel: "Magpie", authorization: account.magpieAuthorization)
                            Spacer(minLength: 0)
                        }
                        Text("当前使用 \(account.activeSourceLabel)")
                            .font(.system(size: 10))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                        if let problem = account.authorizationProblem {
                            Text(problem)
                                .font(.system(size: 10))
                                .foregroundStyle(ConsoleTheme.tone(.warning))
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }

                Divider().overlay(ConsoleTheme.border)

                HStack(spacing: 14) {
                    // 国际版没有签到这项任务：不显示这一栏，而不是写"暂不支持"
                    // —— 那读起来像"以后会有"。
                    if account.supportsCNTasks {
                        labelValue("签到", account.checkinEnabled ? "已开启" : "未开启")
                    }
                    labelValue("最近更新", shortTimestamp(account.updatedAt))
                    Spacer(minLength: 0)
                }
            }
        }
        .contextMenu {
            Button("复制 UID") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(account.uid, forType: .string)
            }
            Button("修改备注") {}.disabled(true)
            Button("重新登录", action: onRelogin)
            Button("删除账号") {}.disabled(true)
        }
    }

    private func labelValue(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Text(value)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.text)
                .lineLimit(1)
        }
    }

    /// `2026-11-09T14:22:31Z` → `11-09 14:22`（本地时区）。
    private func shortTimestamp(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "—" }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime]
        guard let date = parser.date(from: value) else { return String(value.prefix(10)) }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }
}

/// 一份授权的徽章：谁、什么状态、什么时候到期。
///
/// 两份都在时并排显示，用户能一眼看出"Magpie 有、自有没有"这种情况；
/// 只有一份时另一枚显示"未授权"，而不是干脆不画（否则看起来像只有一种来源）。
private struct AuthorizationChip: View {
    let kindLabel: String
    let authorization: ConsoleAuthorization?

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(tint)
                .frame(width: 5, height: 5)
            Text(kindLabel)
                .foregroundStyle(ConsoleTheme.secondaryText)
            Text(detail)
                .foregroundStyle(ConsoleTheme.tertiaryText)
        }
        .font(.system(size: 9, weight: .medium))
        .lineLimit(1)
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 5))
    }

    private var tint: Color {
        guard let authorization else { return ConsoleTheme.tertiaryText }
        return ConsoleTheme.tone(authorization.tone)
    }

    private var detail: String {
        guard let authorization else { return "未授权" }
        if authorization.isMigrating { return "迁移中" }
        if let expires = authorization.tokenExpiresAt, expires.count >= 10 {
            return "\(authorization.authLabel) · \(expires.prefix(10))"
        }
        return authorization.authLabel
    }
}
