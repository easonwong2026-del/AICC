import SwiftUI

/// Picks which quota windows the widgets draw.  A row is one window of one
/// account; the order of the ticks is the order on screen.  Ticking nothing
/// means "decide for me", which is what the widget did before this screen.
///
/// 交互与语义未改，只换成 UI V5 的排版（分组容器 + 单层行）。
struct QuotaSelectionView: View {
    let port: Int
    @Binding var selection: [String]
    let onApply: ([String]) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var snapshot: QuotaSnapshot?
    @State private var loading = false
    @State private var failure: String?
    @State private var picked: [String] = []

    private struct Row: Identifiable {
        let key: String
        let provider: String
        let account: String
        let window: String
        let value: String
        var id: String { key }
    }

    private var rows: [Row] {
        guard let snapshot else { return [] }
        return snapshot.accounts.flatMap { card in
            card.items.map { item in
                Row(key: card.key(for: item), provider: card.providerName, account: card.accountLabel,
                    window: item.name, value: item.primaryText)
            }
        }
    }

    private var grouped: [(provider: String, rows: [Row])] {
        var order: [String] = []
        var byProvider: [String: [Row]] = [:]
        for row in rows {
            if byProvider[row.provider] == nil { order.append(row.provider) }
            byProvider[row.provider, default: []].append(row)
        }
        return order.map { ($0, byProvider[$0] ?? []) }
    }

    private var missingSelection: Int {
        let available = Set(rows.map(\.key))
        return picked.filter { !available.contains($0) }.count
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header

            Text("勾选要显示的额度窗口，顺序即显示顺序。不勾选＝自动按 Magpie 顺序显示。")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, ConsoleTheme.Sheet.padding)
                .padding(.bottom, 10)

            Divider()

            content
                .frame(maxHeight: .infinity)

            if snapshot != nil && !rows.isEmpty && missingSelection > 0 {
                ConsoleInlineNotice(
                    kind: .warning,
                    message: "有 \(missingSelection) 项在 Magpie 里已不存在（窗口改名或账号移除），应用时会自动去掉。"
                )
                .padding(.horizontal, ConsoleTheme.Sheet.padding)
                .padding(.bottom, 10)
            }

            Divider()

            footer
        }
        .background(ConsoleTheme.canvas)
        .frame(width: 480, height: 540)
        .task { await load() }
        .onAppear { picked = selection }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("选择显示的额度")
                    .font(ConsoleType.detailTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text("只影响桌面小组件，不改变 Magpie 的额度数据")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer()
            Button("关闭") { dismiss() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
        }
        .padding(.horizontal, ConsoleTheme.Sheet.padding)
        .padding(.vertical, 14)
    }

    @ViewBuilder private var content: some View {
        if loading {
            VStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("读取 Magpie 额度…")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let failure {
            ConsoleEmptyState(
                symbol: "exclamationmark.triangle",
                title: "读不到额度",
                message: failure,
                actionTitle: "重试",
                action: { Task { await load() } }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if rows.isEmpty {
            ConsoleEmptyState(
                symbol: "square.stack.3d.up.slash",
                title: "Magpie 暂无额度",
                message: "先在 Magpie 登录服务商，这里才有可选的额度窗口。",
                actionTitle: "重新读取",
                action: { Task { await load() } }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                    if !picked.isEmpty {
                        Button("清空选择（恢复自动）") { picked = [] }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                    ForEach(grouped, id: \.provider) { group in
                        SettingsGroup(title: group.provider) {
                            ForEach(Array(group.rows.enumerated()), id: \.element.id) { index, row in
                                if index > 0 { SettingsRowDivider() }
                                rowView(row)
                            }
                        }
                    }
                }
                .padding(.horizontal, ConsoleTheme.Sheet.padding)
                .padding(.vertical, 14)
            }
        }
    }

    private func rowView(_ row: Row) -> some View {
        let selected = picked.contains(row.key)
        let order = picked.firstIndex(of: row.key).map { $0 + 1 }
        return Button {
            toggle(row.key)
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: selected ? "checkmark.square.fill" : "square")
                    .font(.system(size: 12))
                    .foregroundStyle(selected ? ConsoleTheme.accent : ConsoleTheme.tertiaryText)

                VStack(alignment: .leading, spacing: 2) {
                    Text(row.window)
                        .font(ConsoleType.rowTitle)
                        .foregroundStyle(ConsoleTheme.text)
                        .lineLimit(1)
                    Text(row.account)
                        .font(ConsoleType.micro)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }

                Spacer(minLength: 8)

                Text(row.value)
                    .font(ConsoleType.monoSmall)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .lineLimit(1)

                if let order {
                    Text("\(order)")
                        .font(ConsoleType.micro)
                        .monospacedDigit()
                        .foregroundStyle(ConsoleTheme.accent)
                        .frame(minWidth: 12, alignment: .trailing)
                }
            }
            .padding(.vertical, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var footer: some View {
        HStack {
            Text(picked.isEmpty ? "当前：自动" : "当前：已选 \(picked.count) 项")
                .font(ConsoleType.small)
                .foregroundStyle(ConsoleTheme.secondaryText)
            Spacer()
            Button("取消") { dismiss() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
            Button("应用") {
                let available = Set(rows.map(\.key))
                onApply(picked.filter { available.contains($0) })
                dismiss()
            }
            .buttonStyle(ConsolePrimaryButtonStyle())
            .keyboardShortcut(.defaultAction)
            // 读不到列表时不能"应用"：那等于把用户的选择清空。
            .disabled(loading || failure != nil)
        }
        .padding(.horizontal, ConsoleTheme.Sheet.padding)
        .padding(.vertical, 12)
    }

    private func toggle(_ key: String) {
        if let index = picked.firstIndex(of: key) {
            picked.remove(at: index)
        } else {
            picked.append(key)
        }
    }

    private func load() async {
        loading = true
        failure = nil
        defer { loading = false }
        let snapshot = await WidgetStatusLoader.snapshot(port: port)
        if snapshot.accounts.isEmpty {
            failure = "没有读到额度。确认 Magpie 正在运行（端口 \(port)）。"
        } else {
            self.snapshot = snapshot
        }
    }
}
