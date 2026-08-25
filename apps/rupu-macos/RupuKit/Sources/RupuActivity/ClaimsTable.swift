import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

private enum ClaimsTableLayout {
    static let status: CGFloat = 96
    static let repo: CGFloat = 150
    static let workflow: CGFloat = 170
    static let updated: CGFloat = 88
    static let actions: CGFloat = 60
}

/// The Activity screen's autoflows-kind "Claims" sub-tab table (Phase 6B,
/// Task 3) — every tracked autoflow issue claim (`GET /api/autoflows/
/// claims`), with row-level Release/Requeue mutations. Built on `List` (not
/// SwiftUI's stock `Table`), same rationale `ActivityTable`'s own doc
/// comment gives: a per-row conditional busy/disabled state driven off
/// `ClaimsStore.pendingActions` has no supported `Table` hook on macOS.
///
/// **`issueRef` is the `List`/`ForEach` identity** — see `ClaimsStore`'s doc
/// comment for the full uniqueness argument (`AutoflowClaimStore.save`
/// always writes a claim into the directory derived from its OWN
/// `issue_ref`, so `list()` can never return two rows sharing one).
///
/// **Release is confirm-first** (`confirmationDialog` — it forgets the
/// tracked claim entirely, though the server's own delete is idempotent);
/// **requeue is not** (it only enqueues a wake — nothing to lose by firing
/// it directly). Both route through `ClaimsStore.release(issueRef:)`/
/// `requeue(issueRef:)` — see that store's doc comment for why their
/// pending-state confirmation contrasts with a run mutation's.
///
/// **Footer discloses TWO limits, not one** (review fix, round 1 — the
/// original footer only named the host limit): `CPClient.autoflowClaims()`'s
/// doc comment notes claims stay local-only, unlike the `/api/runs*`
/// firehose routes — every row here describes the local `cp serve`'s own
/// claim store, never a merged multi-host view. Separately, this table also
/// silently ignores the top bar's project-scope selection — NOT a filtering
/// bug, a genuine impossibility: see `ClaimsStore`'s "No project-scope
/// filtering" doc comment section for why `repoRef` can't be mapped to a
/// `ws_id` client-side. Both limits are named plainly in one footer line
/// (honest-UI rule), same idiom `FleetScreen.footerNote` uses for its own
/// CLI-only disclosure, rather than an unqualified table that implies either
/// fleet-wide coverage or project-scoped narrowing it doesn't actually have.
struct ClaimsTable: View {
    let rows: [APIClaimRow]
    let store: ClaimsStore

    @State private var pendingRelease: APIClaimRow?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            List(rows, id: \.issueRef) { row in
                ClaimTableRow(row: row, store: store, onRequestRelease: { pendingRelease = row })
                    .listRowSeparator(.visible)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            Divider()
            footerNote
        }
        .panelStyle(.panel)
        .confirmationDialog(
            "Release \(pendingRelease.map(ClaimTableRow.issueLabel) ?? "this claim")?",
            isPresented: releaseDialogBinding,
            presenting: pendingRelease
        ) { row in
            Button("Release", role: .destructive) {
                let issueRef = row.issueRef
                pendingRelease = nil
                Task { await store.release(issueRef: issueRef) }
            }
            Button("Cancel", role: .cancel) { pendingRelease = nil }
        } message: { row in
            Text("This forgets the tracked claim for \(ClaimTableRow.issueLabel(row)) — a future dispatch can re-claim the issue from scratch. This does not touch the issue itself, any open PR, or a run already in flight.")
        }
    }

    private var releaseDialogBinding: Binding<Bool> {
        Binding(get: { Self.isReleaseDialogPresented(pendingRelease) }, set: { if !$0 { pendingRelease = nil } })
    }

    /// The confirm-dialog presence flag, pulled out as a pure static seam
    /// (same "view-member pure logic gets its own testable static func"
    /// idiom `RunDetailScreen.unrecognizedStatusRaw` already establishes) so
    /// `ClaimsTableTests` can assert it directly without standing up a real
    /// SwiftUI render pass: `nil` (no row currently queued for release) is
    /// never presented; any row IS.
    static func isReleaseDialogPresented(_ pendingRelease: APIClaimRow?) -> Bool {
        pendingRelease != nil
    }

    private var header: some View {
        HStack(spacing: 0) {
            headerCell("Status", width: ClaimsTableLayout.status)
            headerCell("Issue", width: nil)
            headerCell("Repo", width: ClaimsTableLayout.repo)
            headerCell("Workflow", width: ClaimsTableLayout.workflow)
            headerCell("Updated", width: ClaimsTableLayout.updated, alignment: .trailing)
            headerCell("", width: ClaimsTableLayout.actions, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    @ViewBuilder
    private func headerCell(_ title: String, width: CGFloat?, alignment: Alignment = .leading) -> some View {
        if let width {
            Eyebrow(title).frame(width: width, alignment: alignment).padding(.trailing, 8)
        } else {
            Eyebrow(title).frame(maxWidth: .infinity, alignment: alignment).padding(.trailing, 8)
        }
    }

    private var footerNote: some View {
        Text("Project scope doesn't apply to claims — always all projects, local host only")
            .font(.noteText)
            .foregroundStyle(Color.rupuMute)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
    }
}

/// One claim: a fixed-column identity line (status / issue / repo /
/// workflow / updated / requeue+release) plus a free-form detail line
/// (owner, lease, last error-or-summary, PR chip) — the same "fixed columns
/// for the scannable identity, a second line for the noisier detail fields"
/// split `HostCard` (`RupuFleet/FleetScreen.swift`) already uses for a
/// similarly field-heavy row.
/// Not `private` (unlike `ActivityTableRow`) — its pure static seams below
/// are exercised directly by `ClaimsTableTests` (`RupuActivityTests`, a
/// different file AND a different target) via `@testable import
/// RupuActivity`, which only reaches `internal`, never file-scoped
/// `private`, declarations.
struct ClaimTableRow: View {
    let row: APIClaimRow
    let store: ClaimsStore
    let onRequestRelease: () -> Void

    private var releaseKey: ActionKey { ActionKey(row.issueRef, .release) }
    private var requeueKey: ActionKey { ActionKey(row.issueRef, .requeue) }

    private var isReleasePending: Bool {
        if case .pending = store.pendingActions.state(releaseKey) { return true }
        return false
    }
    private var isRequeuePending: Bool {
        if case .pending = store.pendingActions.state(requeueKey) { return true }
        return false
    }
    private var isBusy: Bool { isReleasePending || isRequeuePending }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            identityLine
            detailLine
            if case .failed(let message) = store.pendingActions.state(releaseKey) {
                failureNote("Release failed: \(message)")
            }
            if case .failed(let message) = store.pendingActions.state(requeueKey) {
                failureNote("Requeue failed: \(message)")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .opacity(isBusy ? 0.6 : 1)
    }

    private var identityLine: some View {
        HStack(spacing: 0) {
            statusCell.frame(width: ClaimsTableLayout.status, alignment: .leading).padding(.trailing, 8)
            issueCell.frame(maxWidth: .infinity, alignment: .leading).padding(.trailing, 8)
            Text(row.repoRef)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: ClaimsTableLayout.repo, alignment: .leading)
                .padding(.trailing, 8)
            Text(row.workflow)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: ClaimsTableLayout.workflow, alignment: .leading)
                .padding(.trailing, 8)
            Text(Self.updatedLabel(row))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: ClaimsTableLayout.updated, alignment: .trailing)
            actions.frame(width: ClaimsTableLayout.actions, alignment: .trailing)
        }
    }

    @ViewBuilder
    private var issueCell: some View {
        let label = Self.issueLabel(row)
        if let urlString = row.issueURL, let url = URL(string: urlString) {
            Button {
                NSWorkspace.shared.open(url)
            } label: {
                Text(label)
                    .foregroundStyle(Color.rupuBrand700)
                    .underline()
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .buttonStyle(.plain)
            .help(urlString)
        } else {
            Text(label)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
        }
    }

    private var statusCell: some View {
        HStack(spacing: 6) {
            Circle().fill(Color.status(Self.claimTone(row.status))).frame(width: 6, height: 6)
            Text(Self.claimStatusLabel(row.status))
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
        }
    }

    /// Owner/lease render `—` when nil (honest-UI rule); the last
    /// error-or-summary note (error takes priority — a claim with a
    /// `last_error` is the more urgent thing to surface) is truncated to a
    /// single line with the full text in `.help(_:)`; the PR chip only
    /// appears when `prURL` is present.
    private var detailLine: some View {
        HStack(spacing: 10) {
            Text("Owner: \(Self.ownerLabel(row))")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
            Text("Lease: \(Self.leaseLabel(row))")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
            if let note = Self.noteText(row) {
                Text(note.text)
                    .font(.noteText)
                    .foregroundStyle(note.isError ? Color.status(.failed) : Color.rupuMute)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(note.text)
            }
            if let prURLString = row.prURL, let prURL = URL(string: prURLString) {
                Button {
                    NSWorkspace.shared.open(prURL)
                } label: {
                    Text("PR")
                        .font(.metaText)
                        .foregroundStyle(Color.rupuBrand700)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Color.rupuBrand.opacity(0.12))
                        .clipShape(ChromeShape.pill)
                }
                .buttonStyle(.plain)
                .help(prURLString)
            }
            Spacer(minLength: 0)
        }
    }

    private var actions: some View {
        HStack(spacing: 8) {
            Button {
                Task { await store.requeue(issueRef: row.issueRef) }
            } label: {
                if isRequeuePending {
                    ProgressView().controlSize(.mini)
                } else {
                    Icon(.repeatIcon, size: 13)
                }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Color.rupuDim)
            .disabled(isBusy)
            .help("Requeue")

            Button {
                onRequestRelease()
            } label: {
                if isReleasePending {
                    ProgressView().controlSize(.mini)
                } else {
                    Icon(.trash2, size: 13)
                }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Color.status(.failed))
            .disabled(isBusy)
            .help("Release")
        }
    }

    private func failureNote(_ message: String) -> some View {
        Text(message)
            .font(.noteText)
            .foregroundStyle(Color.status(.failed))
            .lineLimit(1)
            .truncationMode(.tail)
            .help(message)
    }

    // MARK: - Pure static seams (testable via `@testable import RupuActivity`
    // from `ClaimsTableTests`, `RupuActivityTests` — same "view-member pure
    // logic gets its own testable static func" idiom
    // `RunDetailScreen.unrecognizedStatusRaw` already establishes, since a
    // SwiftUI `body` itself can't be meaningfully unit-rendered).

    /// `row.issueDisplayRef ?? row.issueRef` — the issue cell's label,
    /// whether or not `issueURL` is present to make it tappable.
    static func issueLabel(_ row: APIClaimRow) -> String {
        row.issueDisplayRef ?? row.issueRef
    }

    /// `—` when `claimOwner` is nil (honest-UI rule — never a blank cell).
    static func ownerLabel(_ row: APIClaimRow) -> String {
        row.claimOwner ?? "—"
    }

    /// `—` when `leaseExpiresAt` is nil OR fails to parse (an unparseable
    /// timestamp is treated the same as "no lease info" rather than
    /// crashing or silently showing a raw ISO string) — `now` is injectable
    /// so tests can assert a fixed relative string deterministically.
    static func leaseLabel(_ row: APIClaimRow, now: Date = Date()) -> String {
        guard let raw = row.leaseExpiresAt, let date = ActivityRow.parseISO(raw) else { return "—" }
        return relativeFormatter.localizedString(for: date, relativeTo: now)
    }

    /// `—` when `updatedAt` fails to parse — every fixture/live row carries
    /// one (`APIClaimRow.updatedAt` is non-optional), but a malformed value
    /// is still handled honestly rather than crashing.
    static func updatedLabel(_ row: APIClaimRow, now: Date = Date()) -> String {
        guard let date = ActivityRow.parseISO(row.updatedAt) else { return "—" }
        return relativeFormatter.localizedString(for: date, relativeTo: now)
    }

    /// The detail line's error-or-summary note: `lastError` takes priority
    /// over `lastSummary` (a claim with a `last_error` is the more urgent
    /// thing to surface) — `nil` when both are absent, so the view renders
    /// nothing rather than an empty chip. `isError` drives the row's tone
    /// (fail-red vs muted).
    static func noteText(_ row: APIClaimRow) -> (text: String, isError: Bool)? {
        if let err = row.lastError { return (err, true) }
        if let summary = row.lastSummary { return (summary, false) }
        return nil
    }

    /// `"await_human"` → `"Await Human"` — a plain underscore→space+
    /// title-case pass over the server's own snake_case `ClaimStatus`
    /// vocabulary (`eligible`/`claimed`/`running`/`await_human`/
    /// `await_external`/`retry_backoff`/`blocked`/`complete`/`released` —
    /// `crates/rupu-workspace/src/autoflow_claim.rs`). Deliberately NOT
    /// folded into `ActivityStatus.normalize`/`displayLabel` — that type's
    /// vocabulary is the run/session lifecycle (pending/running/completed/
    /// ...), a different domain a claim's own status doesn't map onto
    /// cleanly (there is no run-shaped equivalent of `retry_backoff` or
    /// `await_external`).
    static func claimStatusLabel(_ status: String) -> String {
        status
            .split(separator: "_")
            .map { $0.prefix(1).uppercased() + $0.dropFirst() }
            .joined(separator: " ")
    }

    /// Best-effort tone mapping onto the shared 9-state `StatusTone` palette
    /// — same "borrow the closest existing tone, don't invent a tenth"
    /// discipline every other status-adjacent color mapping in this module
    /// follows. An unrecognized status (a future `ClaimStatus` variant this
    /// client doesn't know about yet) falls back to `.pending` rather than
    /// guessing something more alarming.
    static func claimTone(_ status: String) -> StatusTone {
        switch status {
        case "eligible": .pending
        case "claimed", "running": .running
        case "await_human", "await_external": .awaiting
        case "retry_backoff": .paused
        case "blocked": .failed
        case "complete": .done
        case "released": .cancelled
        default: .pending
        }
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}
