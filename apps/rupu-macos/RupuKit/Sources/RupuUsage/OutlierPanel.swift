import AppKit
import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

/// Maps an outlier run to the route its row navigates to — a small pure,
/// tested mapping function, same "one file-scoped pure function per
/// navigating table" idiom `findingNavigationRoute`
/// (`RupuSecurity/FindingsTable.swift`) establishes.
///
/// **`host: nil` is honest, not a punt.** `GET /api/usage/outliers` reads
/// `s.run_store` directly, server-side — no host fan-out at all (see
/// `crates/rupu-cp/src/api/usage_outliers.rs`'s `get_usage_outliers`, and
/// its sibling `/api/usage/runs` handler's identical doc comment: "local-
/// only, like `/api/usage/timeline` and `/api/usage/outliers`"). Every
/// `APIOutlierRun.runID` is therefore a LOCAL orchestrator run id, exactly
/// what `.runDetail(id:host: nil)` / `GET /api/runs/:id` (no `?host=`)
/// resolves — unlike `FindingsTable`'s `findingNavigationRoute`, which has
/// to special-case surfaces it genuinely cannot resolve, every outlier row
/// navigates.
func outlierNavigationRoute(runID: String) -> Route {
    .runDetail(id: runID, host: nil)
}

/// The Usage screen's cost-outlier panel (spec §4/brief): runs costing far
/// more than their own workflow's per-run median baseline. Content port of
/// the web's `OutlierPanel.tsx` — same three figures per row (workflow name
/// + run id, cost, ratio × baseline) — MINUS its Task U3 exclude-checkbox
/// interactivity (feeding the spend graph's client-side filter set): out of
/// this task's scope per the brief, which asks only for "run/cost vs
/// baseline/ratio; rows navigate to run detail".
///
/// **Not client-re-sorted.** `find_outliers` (the Rust source behind this
/// endpoint) already sorts its output descending by `ratio` before
/// returning — rendered in the order given, matching the web (which also
/// never re-sorts this list).
struct OutlierPanel: View {
    let outliers: [APIOutlierRun]
    let onSelect: (Route) -> Void

    var body: some View {
        if outliers.isEmpty {
            emptyBlock
        } else {
            VStack(spacing: 0) {
                // `id: \.runID` (backlog row 20 fix), not `\.offset` — an
                // offset id ties a row's SwiftUI identity to its array
                // position, so a window change that re-fetches a
                // differently-ordered (or partially overlapping) outlier set
                // would reassign row identity to whatever run lands on the
                // old offset instead of following the run it actually
                // belongs to. `runID` is already a stable, locally-unique
                // key for this endpoint (see this type's own doc comment:
                // local-only, no host fan-out), so no composite is needed.
                ForEach(outliers, id: \.runID) { row in
                    OutlierRow(outlier: row, onSelect: { onSelect(outlierNavigationRoute(runID: row.runID)) })
                    Divider()
                }
            }
            .panelStyle(.panel)
        }
    }

    private var emptyBlock: some View {
        HStack {
            Spacer(minLength: 0)
            Text("No cost outliers in this window")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 80)
        .panelStyle(.panel)
    }
}

private struct OutlierRow: View {
    let outlier: APIOutlierRun
    let onSelect: () -> Void

    private var ratioLabel: String {
        "\(String(format: "%.1f", outlier.ratio))× baseline (\(Fmt.cost(outlier.baselineUSD)))"
    }

    var body: some View {
        rowContent
            .contentShape(Rectangle())
            .onTapGesture(perform: onSelect)
            .onHover { hovering in
                if hovering {
                    NSCursor.pointingHand.push()
                } else {
                    NSCursor.pop()
                }
            }
    }

    private var rowContent: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(outlier.workflowName)
                    .font(.noteText.weight(.semibold))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Text(outlier.runID)
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Text(Fmt.cost(outlier.costUSD))
                .font(.dataMono(12))
                .foregroundStyle(Color.rupuInk)

            Text(ratioLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.status(.failed))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}
