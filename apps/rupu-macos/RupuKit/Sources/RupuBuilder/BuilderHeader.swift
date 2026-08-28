import SwiftUI
import RupuDesign

/// The Workflow Builder screen's fixed 46pt header row (macOS design plan,
/// Task 10, spec §1): back chevron + breadcrumb, a "running" `StatusPill`
/// while Run mode is following a live run, a valid/invalid dot, the
/// Design/Run segmented control, the YAML-source toggle, Save (dirty dot +
/// ⌘S), and Launch. Pulled into its own file per the task's file list —
/// `WorkflowBuilderScreen` owns the `BuilderStore` this reads/writes and
/// composes this above the canvas/YAML pane/rail.
struct BuilderHeader: View {
    @Bindable var store: BuilderStore
    let name: String
    @Binding var sourceOpen: Bool
    let onBack: () -> Void
    let onSave: () -> Void
    let onLaunch: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Button(action: onBack) {
                Icon(.arrowLeft)
                    .foregroundStyle(Color.rupuDim)
            }
            .buttonStyle(.plain)

            breadcrumb

            if hasLiveRun {
                StatusPill(.running, compact: true)
            }

            Spacer(minLength: 12)

            validDot
            modePicker
            sourceToggleButton
            saveButton

            Button("Launch", action: onLaunch)
                .buttonStyle(RupuButtonStyle.builderLaunch)
        }
        .padding(.horizontal, 12)
        .frame(height: 46)
        .frame(maxWidth: .infinity)
        .background(Color.rupuPanel)
    }

    private var breadcrumb: some View {
        HStack(spacing: 0) {
            Text("Library ▸ ")
                .font(.leadText)
                .foregroundStyle(Color.rupuDim)
            Text(name)
                .font(.leadText.weight(.semibold))
                .foregroundStyle(Color.rupuInk)
        }
        .lineLimit(1)
    }

    /// Whether the "running" `StatusPill` should render — true exactly when
    /// Run mode is actively following a live run. `BuilderStore.
    /// enterRunMode(client:backend:)` (Task 14) is what will give this
    /// screen a real "is a run actually live" signal; no such state exists
    /// on `BuilderStore` yet, so this always resolves `false` today rather
    /// than approximating with `store.mode == .run` (which would render a
    /// "running" pill even when Run mode has nothing to follow — a mocked
    /// state the no-mock-features rule forbids).
    private var hasLiveRun: Bool { false }

    private var validDot: some View {
        let tone = validDotTone(serverValid: store.serverValid, problems: store.problems)
        return HStack(spacing: 5) {
            Circle()
                .fill(tone.map { Color.status($0) } ?? Color.rupuMute)
                .frame(width: 8, height: 8)
            Text(tone == .failed ? "invalid" : "valid")
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
        }
    }

    private var modePicker: some View {
        Picker("Mode", selection: $store.mode) {
            Text("Design").tag(BuilderStore.Mode.design)
            Text("Run").tag(BuilderStore.Mode.run)
        }
        .pickerStyle(.segmented)
        .frame(width: 130)
        .labelsHidden()
    }

    private var sourceToggleButton: some View {
        Button {
            sourceOpen.toggle()
        } label: {
            Icon(.code)
                .foregroundStyle(sourceOpen ? Color.rupuInk : Color.rupuDim)
        }
        .buttonStyle(.plain)
        .help(sourceOpen ? "Hide YAML source" : "Show YAML source")
    }

    private var saveButton: some View {
        Button(action: onSave) {
            HStack(spacing: 6) {
                if store.dirty {
                    Circle().fill(Color.rupuBrand).frame(width: 6, height: 6)
                }
                Text("Save")
            }
        }
        .buttonStyle(RupuButtonStyle.outline)
        .keyboardShortcut("s", modifiers: .command)
    }
}

/// Pure helper backing the header's valid dot (spec §1) — extracted so its
/// three cases are assertable without a SwiftUI render pass. `nil` while
/// `serverValid` hasn't been checked yet (the dot renders `rupuMute`, no
/// tone); `.done` once the last server check passed AND there are no local
/// graph-validation `problems` outstanding; `.failed` for every other case
/// (a failed server check, or a passed one with local `problems` the
/// debounce hasn't caught up to yet — `problems` always wins pessimistically).
func validDotTone(serverValid: Bool?, problems: [String: [String]]) -> StatusTone? {
    guard let serverValid else { return nil }
    return serverValid && problems.isEmpty ? .done : .failed
}
