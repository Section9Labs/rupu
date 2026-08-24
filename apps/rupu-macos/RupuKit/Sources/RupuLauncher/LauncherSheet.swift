import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Launcher sheet (HANDOFF screen 9): kind segmented → `DefinitionPicker`
/// → a free-text prompt (`.agentRun`/`.session`) or `InputsForm`
/// (`.workflow`) → a mode segmented control (the `bypass` segment tinted
/// `Color.status(.fail)` — loud rule, since bypass skips every permission
/// gate) → `HostChips` → a footer with the Launch button and, once
/// `launch()` has fired, per-host outcome rows that land progressively as
/// each target's POST resolves.
///
/// Owns the `LauncherStore`'s lifecycle exactly like `ActivityScreen` owns
/// `ActivityStore`'s: built lazily in `.task` once `backend.client()`
/// exists, one instance per sheet presentation (never reused across
/// re-opens — see `LauncherStore`'s doc comment on why that reset is
/// deliberate).
public struct LauncherSheet: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @Environment(\.dismiss) private var dismiss

    @State private var store: LauncherStore?

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
    }

    public var body: some View {
        Group {
            if let store {
                LauncherForm(model: model, store: store, dismiss: dismiss)
            } else {
                connectingView
            }
        }
        .frame(width: 560)
        .task {
            await activate()
        }
    }

    private var connectingView: some View {
        VStack(spacing: 8) {
            ProgressView().controlSize(.small)
            MicroLabel("CONNECTING")
                .foregroundStyle(Color.rupuMute)
        }
        .frame(width: 560, height: 320)
    }

    /// Guards against rebuilding the store on a `.task` re-run (e.g. a
    /// scene reactivation while the sheet is still up) — `store == nil` is
    /// the same one-shot-construction contract `ActivityScreen.activate`
    /// documents.
    private func activate() async {
        guard store == nil else { return }
        guard let client = backend.client() else { return }
        let newStore = LauncherStore(client: client)
        store = newStore
        await newStore.activate()
    }
}

/// The sheet's actual form body, split out from `LauncherSheet` so it can
/// hold a non-optional `@Bindable var store` (mirrors `ActivityScreen` /
/// `FilterBar`'s split: the screen owns the optional, the content view
/// underneath declares the unwrapped `@Bindable`).
private struct LauncherForm: View {
    @Bindable var model: AppModel
    @Bindable var store: LauncherStore
    let dismiss: DismissAction

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                kindPicker
                DefinitionPicker(store: store)
                promptOrInputs
                modePicker
                HostChips(store: store, localOnly: store.kind == .session)
                footer
            }
            .padding(20)
        }
        .frame(width: 560, height: 600)
    }

    private var header: some View {
        Text("New run")
            .font(.system(size: 15, weight: .semibold))
            .foregroundStyle(Color.rupuInk)
    }

    private var kindPicker: some View {
        Picker("Kind", selection: $store.kind) {
            Text("Agent run").tag(LaunchKind.agentRun)
            Text("Session").tag(LaunchKind.session)
            Text("Workflow").tag(LaunchKind.workflow)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .onChange(of: store.kind) { _, _ in
            // A different kind means a different definition namespace
            // (agents vs workflows) — the prior selection can never be
            // valid for the new kind, so it's cleared rather than left
            // dangling. `canLaunch` requiring a non-nil
            // `selectedDefinition` means Launch stays disabled until the
            // operator picks one from the now-current list.
            store.selectedDefinition = nil
        }
    }

    @ViewBuilder
    private var promptOrInputs: some View {
        if store.kind == .workflow {
            VStack(alignment: .leading, spacing: 6) {
                MicroLabel("Inputs")
                    .foregroundStyle(Color.rupuMute)
                if store.selectedDefinition == nil {
                    MicroLabel("SELECT A WORKFLOW TO SEE ITS INPUTS")
                        .foregroundStyle(Color.rupuMute)
                } else {
                    InputsForm(store: store)
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 6) {
                MicroLabel("Prompt")
                    .foregroundStyle(Color.rupuMute)
                TextEditor(text: $store.prompt)
                    .font(.system(size: 12.5))
                    .scrollContentBackground(.hidden)
                    .frame(height: 90)
                    .padding(6)
                    .panelStyle(.innerCard)
            }
        }
    }

    private var modePicker: some View {
        VStack(alignment: .leading, spacing: 6) {
            MicroLabel("Mode")
                .foregroundStyle(Color.rupuMute)
            HStack(spacing: 6) {
                modeSegment("ask", label: "Ask")
                modeSegment("readonly", label: "Read-only")
                modeSegment("bypass", label: "Bypass", loud: true)
            }
        }
    }

    /// One mode segment. `loud` (only `"bypass"`) always carries
    /// `Color.status(.fail)` — as text/border when unselected, as a solid
    /// fill when selected — so bypass reads as alarming regardless of
    /// selection state, never just another neutral segment choice.
    private func modeSegment(_ value: String, label: String, loud: Bool = false) -> some View {
        let isSelected = store.mode == value
        let tint = loud ? Color.status(.fail) : Color.rupuBrand
        return Button {
            store.mode = value
        } label: {
            Text(label)
                .font(.system(size: 11.5, weight: loud ? .semibold : .regular))
                .foregroundStyle(labelColor(isSelected: isSelected, loud: loud))
                .frame(maxWidth: .infinity)
                .padding(.vertical, 6)
                .background(isSelected ? tint : (loud ? Color.status(.fail).opacity(0.1) : Color.rupuSurface))
                .clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(loud ? Color.status(.fail) : Color.rupuBorder, lineWidth: loud ? 1.2 : 1)
                )
        }
        .buttonStyle(.plain)
    }

    private func labelColor(isSelected: Bool, loud: Bool) -> Color {
        if isSelected { return .white }
        return loud ? Color.status(.fail) : Color.rupuInk
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let error = store.validationError {
                HStack(spacing: 4) {
                    Image(systemName: "exclamationmark.circle.fill")
                        .foregroundStyle(Color.status(.fail))
                    Text(error)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Color.status(.fail))
                }
            }

            if !store.launchResults.isEmpty {
                outcomesList
            }

            HStack {
                Spacer()
                launchButton
            }
        }
    }

    private var outcomesList: some View {
        VStack(alignment: .leading, spacing: 4) {
            MicroLabel("Results")
                .foregroundStyle(Color.rupuMute)
            ForEach(store.launchResults, id: \.host) { outcome in
                outcomeRow(outcome)
            }
        }
    }

    /// A success row is a button navigating to that target's destination —
    /// per host, since a multi-target fan-out lands one `Route` per target
    /// and each is independently worth jumping to. A failure row is inert
    /// text: `error.text`, never a control that looks actionable but isn't.
    @ViewBuilder
    private func outcomeRow(_ outcome: LaunchOutcome) -> some View {
        switch outcome.result {
        case .success(let route):
            Button {
                model.navigate(to: route)
                dismiss()
            } label: {
                HStack(spacing: 6) {
                    Circle().fill(Color.status(RunTone.done)).frame(width: 6, height: 6)
                    Text(outcome.host)
                        .font(.identifier)
                        .foregroundStyle(Color.rupuInk)
                    Spacer()
                    Text("view →")
                        .font(.system(size: 11))
                        .foregroundStyle(Color.rupuBrand600)
                }
            }
            .buttonStyle(.plain)
        case .failure(let error):
            HStack(spacing: 6) {
                Circle().fill(Color.status(.fail)).frame(width: 6, height: 6)
                Text(outcome.host)
                    .font(.identifier)
                    .foregroundStyle(Color.rupuInk)
                Spacer()
                Text(error.text)
                    .font(.system(size: 11))
                    .foregroundStyle(Color.status(.fail))
                    .multilineTextAlignment(.trailing)
            }
        }
    }

    private var launchButton: some View {
        let pending = isPending(store.pendingActions.state(ActionKey("launcher", .launch)))
        return Button {
            Task {
                if let route = await store.launch() {
                    // Single-target success: navigate and close — the only
                    // case `launch()` returns a non-nil `Route` for (see
                    // its doc comment). Every other outcome (multi-target,
                    // or a single target that failed) leaves the sheet open
                    // so the operator reads `launchResults` below instead.
                    model.navigate(to: route)
                    dismiss()
                }
            }
        } label: {
            HStack(spacing: 6) {
                if pending {
                    ProgressView().controlSize(.mini)
                }
                Text("Launch")
            }
            .frame(minWidth: 80)
        }
        .buttonStyle(.borderedProminent)
        .disabled(!store.canLaunch || pending)
    }

    private func isPending(_ state: ActionState) -> Bool {
        if case .pending = state { return true }
        return false
    }
}
