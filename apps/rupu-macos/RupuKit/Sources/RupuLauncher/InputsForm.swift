import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Renders one editable row per `store.workflowInputs` entry — a required
/// marker next to the label, a values popup (`Picker`, `.menu` style) when
/// the declared input restricts to `allowedValues`, a plain `TextField`
/// otherwise. Rows are sorted by key for a stable, deterministic order (the
/// backing store is a `[String: WorkflowInputDef]`, which has none).
///
/// `store.inputsLoadError` — set when the `GET /api/workflows/:name` fetch
/// behind a workflow selection failed — renders as its own warning above
/// the rows rather than blocking them: the store fails open (`workflowInputs`
/// empty either way), so there's nothing to show error-adjacent to, only a
/// standalone note that the declared-input list may be incomplete.
struct InputsForm: View {
    @Bindable var store: LauncherStore

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let error = store.inputsLoadError {
                HStack(alignment: .top, spacing: 6) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(Color.status(.waiting))
                    VStack(alignment: .leading, spacing: 2) {
                        MicroLabel("COULDN'T LOAD DECLARED INPUTS")
                            .foregroundStyle(Color.status(.waiting))
                        Text(error)
                            .font(.system(size: 11))
                            .foregroundStyle(Color.rupuDim)
                    }
                }
            }

            if sortedKeys.isEmpty {
                MicroLabel("NO DECLARED INPUTS")
                    .foregroundStyle(Color.rupuMute)
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(sortedKeys, id: \.self) { key in
                        if let def = store.workflowInputs[key] {
                            inputRow(key: key, def: def)
                        }
                    }
                }
            }
        }
    }

    private var sortedKeys: [String] {
        store.workflowInputs.keys.sorted()
    }

    private func inputRow(key: String, def: WorkflowInputDef) -> some View {
        HStack(alignment: .center, spacing: 10) {
            HStack(spacing: 3) {
                MicroLabel(key)
                    .foregroundStyle(Color.rupuInk)
                if def.required {
                    Text("*")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(Color.status(.fail))
                }
            }
            .frame(width: 140, alignment: .leading)

            if def.allowedValues.isEmpty {
                TextField(def.type, text: valueBinding(key))
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12))
            } else {
                Picker("", selection: valueBinding(key)) {
                    ForEach(def.allowedValues, id: \.self) { value in
                        Text(value).tag(value)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
            }
        }
    }

    private func valueBinding(_ key: String) -> Binding<String> {
        Binding(
            get: { store.inputValues[key] ?? "" },
            set: { store.inputValues[key] = $0 }
        )
    }
}
