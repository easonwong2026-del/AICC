import SwiftUI

struct CodexCard: View {
    @EnvironmentObject private var settings: AppSettings
    let codex: CodexData
    let snapshot: WidgetDisplaySnapshot

    var body: some View {
        VStack(spacing: 8) {
            if let accounts = codex.accounts {
                poolSection(accounts)
            } else if let weekly = codex.weekly, let remaining = snapshot.codexWeeklyRemaining {
                weeklySection(weekly: weekly, remaining: remaining)
            } else if let fiveHour = codex.five_hour, let remaining = snapshot.codexFiveHourRemaining {
                fiveHourOnlySection(fiveHour: fiveHour, remaining: remaining)
            } else {
                placeholderContent
            }
        }
        .frame(maxWidth: .infinity)
        .overlay(alignment: .topLeading) {
            if codex.stale == true || snapshot.codexState == "stale" {
                Text("缓存").font(.system(size: 9)).foregroundColor(.orange).offset(y: -9)
            }
        }
    }

    private func poolSection(_ accounts: [CodexAccount]) -> some View {
        let mode = CodexAccountDisplayMode(rawValue: settings.codexAccountDisplayMode) ?? .automatic
        let displayedAccounts = CodexAccountResolver.resolve(
            accounts: accounts,
            selectionMode: codex.selection_mode,
            activeAccountID: codex.active_account_id,
            displayMode: mode,
            idProvider: { $0.id },
            activeProvider: { $0.active }
        )

        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text("Codex")
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                    .foregroundColor(.secondary)
                Spacer()
                Text(headerSubtitle(accounts: accounts))
                    .font(.system(size: DashboardTypography.timestamp))
                    .foregroundColor(.secondary)
            }
            if displayedAccounts.isEmpty {
                Text("No OpenCodex accounts").foregroundColor(.secondary)
            } else if displayedAccounts.count == 1 {
                accountCard(displayedAccounts[0], showReset: true)
            } else if displayedAccounts.count == 2 {
                HStack(alignment: .top, spacing: 10) {
                    accountCard(displayedAccounts[0])
                    accountCard(displayedAccounts[1])
                }
            } else {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                    ForEach(displayedAccounts.indices, id: \.self) { index in
                        accountCard(displayedAccounts[index])
                    }
                }
            }
        }
    }

    private func headerSubtitle(accounts: [CodexAccount]) -> String {
        let countText = String(format: settings.localized(accounts.count == 1 ? "%d account" : "%d accounts"), accounts.count)
        if codex.selection_mode == "auto" {
            return settings.localized("Automatic") + " · " + countText
        }
        return countText
    }

    private func accountCard(_ account: CodexAccount, showReset: Bool = false) -> some View {
        let weeklyVal = account.weekly?.remaining
        let fiveHourVal = account.five_hour?.remaining
        let primary = weeklyVal ?? fiveHourVal
        let primaryLabel = weeklyVal != nil ? "Weekly" : "5 Hour"
        let secondary = weeklyVal != nil ? fiveHourVal : nil
        let isKnown = primary != nil && primary!.isFinite
        let numText = isKnown ? String(format: "%.0f", primary!) : "—"

        return VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(account.safeDisplayName)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Spacer()
                if account.needs_reauth == true {
                    Text("Reauth")
                        .font(.system(size: 9.5, weight: .medium))
                        .foregroundColor(.orange)
                } else if codex.selection_mode != "auto" && (codex.active_account_id.map { $0 == account.id } ?? (account.active == true)) {
                    HStack(spacing: 2.5) {
                        Circle().fill(Color.green).frame(width: 4.5, height: 4.5)
                        Text("Active")
                            .font(.system(size: 9.5, weight: .medium))
                            .foregroundColor(.green)
                    }
                }
                if account.stale == true {
                    Text(settings.localized("Cached"))
                        .font(.system(size: 8.5))
                        .foregroundColor(.orange)
                }
            }

            HStack(alignment: .lastTextBaseline, spacing: 2) {
                Text(numText)
                    .font(.system(size: 26, weight: .bold, design: .rounded).monospacedDigit())
                    .foregroundColor(isKnown ? progressColor(primary!) : .secondary)
                    .lineLimit(1)
                if isKnown {
                    Text("%")
                        .font(.system(size: DashboardTypography.unit, weight: .medium))
                        .foregroundColor(.secondary)
                }
                Spacer(minLength: 0)
                Text(primaryLabel)
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(Color.primary.opacity(0.1))
                        .frame(height: 4.5)
                    if isKnown {
                        Capsule()
                            .fill(progressColor(primary!))
                            .frame(width: geo.size.width * CGFloat(min(max(primary!, 0), 100) / 100.0), height: 4.5)
                    }
                }
            }
            .frame(height: 4.5)

            HStack {
                if let sec = secondary, sec.isFinite {
                    Text("5h")
                        .font(.system(size: 10))
                        .foregroundColor(.secondary)
                    Text(String(format: "%.0f%%", sec))
                        .font(.system(size: 10, weight: .semibold).monospacedDigit())
                        .foregroundColor(progressColor(sec))
                } else {
                    Text(" ")
                        .font(.system(size: 10))
                }
                Spacer(minLength: 0)
                if showReset, let reset = account.weekly?.reset, !reset.isEmpty {
                    let shortReset = reset.replacingOccurrences(of: "^\\d{4}-", with: "", options: .regularExpression)
                    Text("Weekly Reset " + shortReset)
                        .font(.system(size: 9))
                        .foregroundColor(.secondary)
                        .fixedSize(horizontal: true, vertical: false)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10)
                .fill(Color.primary.opacity(0.04))
        )
    }

    private func weeklySection(weekly: RateWindow, remaining: Double) -> some View {
        VStack(spacing: 6) {
            HStack(alignment: .lastTextBaseline) {
                Text("Codex Weekly")
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                    .foregroundColor(.secondary)
                Spacer()
                quotaNumber(remaining, unit: "%", baseSize: 34)
            }

            progressBar(remaining, height: 6)

            HStack(spacing: 10) {
                if let reset = snapshot.codexWeeklyReset, !reset.isEmpty {
                    Text(resetText(reset))
                        .font(.system(size: DashboardTypography.timestamp))
                        .foregroundColor(.secondary)
                }
                Spacer()
                if let fiveRem = snapshot.codexSecondaryFiveHourRemaining {
                    HStack(alignment: .lastTextBaseline, spacing: 3) {
                        Text("5 Hour")
                            .font(.system(size: DashboardTypography.metricLabel))
                            .foregroundColor(.secondary)
                        Text(String(format: "%.0f", fiveRem))
                            .font(.system(size: DashboardTypography.secondaryMetric, weight: .semibold))
                            .foregroundColor(progressColor(fiveRem))
                        Text("%")
                            .font(.system(size: 10))
                            .foregroundColor(.secondary)
                    }
                }
            }
        }
    }

    private func fiveHourOnlySection(fiveHour: RateWindow, remaining: Double) -> some View {
        VStack(spacing: 6) {
            HStack(alignment: .lastTextBaseline) {
                Text("Codex 5h")
                    .font(.system(size: DashboardTypography.metricLabel, weight: .medium))
                    .foregroundColor(.secondary)
                Spacer()
                quotaNumber(remaining, unit: "%", baseSize: 34)
            }
            progressBar(remaining, height: 6)
            if let reset = snapshot.codexWeeklyReset, !reset.isEmpty {
                Text(resetText(reset))
                    .font(.system(size: DashboardTypography.timestamp))
                    .foregroundColor(.secondary)
            }
        }
    }

    private func quotaNumber(_ value: Double, unit: String, baseSize: CGFloat) -> some View {
        let number = String(format: "%.0f", value)
        return HStack(alignment: .lastTextBaseline, spacing: 3) {
            Text(number)
                .font(.system(
                    size: max(baseSize - 2, DashboardTypography.primaryFontSize(number: number)),
                    weight: .bold
                ))
                .foregroundColor(progressColor(value))
                .lineLimit(1)
            Text(unit)
                .font(.system(size: DashboardTypography.unit, weight: .medium))
                .foregroundColor(.secondary)
        }
    }

    private func progressBar(_ value: Double, height: CGFloat) -> some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: height / 2)
                    .fill(Color.primary.opacity(0.1))
                    .frame(height: height)
                RoundedRectangle(cornerRadius: height / 2)
                    .fill(progressColor(value))
                    .frame(width: geo.size.width * CGFloat(min(max(value, 0), 100) / 100.0), height: height)
            }
        }
        .frame(height: height)
    }

    private func resetText(_ reset: String) -> String {
        String(format: settings.localized("Reset %@"), reset)
    }

    private var placeholderContent: some View {
        HStack {
            Image(systemName: "chart.bar.fill")
                .foregroundColor(.secondary)
            Text("Codex: No data")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
    }

    private func progressColor(_ value: Double) -> Color {
        if value > 70 { return .green }
        if value >= 30 { return .yellow }
        return .red
    }
}
