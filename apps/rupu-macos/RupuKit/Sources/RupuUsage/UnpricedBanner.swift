import RupuAPI
import RupuDesign
import SwiftUI

/// The unpriced-spend banner (spec §4/brief) — content port of the web's
/// `UnpricedBanner.tsx`: names which models had no price mapping and how
/// many token rows they account for, so the headline spend figure reads as
/// an honest UNDER-count rather than a silently-wrong total. Uses `TintBanner`'s
/// warn tone (`Color.status(.awaiting)`), the same tone/toneBg pairing
/// `InputsForm`/`RunDetailScreen`/`OnboardingView` already use for a
/// non-fatal "pay attention" callout — this is not an error, spend just
/// isn't fully known.
///
/// Caller-gated on `!unpriced.models.isEmpty` (mirrors the web component's
/// own internal `if (unpriced.models.length === 0) return null` early-out —
/// ported as a call-site condition here instead of an internal one so this
/// view stays a plain, always-rendering `View` like every other block in
/// this module, consistent with `UsageScreen`'s "one `if` per optional
/// block" composition style).
struct UnpricedBanner: View {
    let unpriced: APIUnpricedGap

    private var modelCountLabel: String {
        "\(unpriced.models.count) model\(unpriced.models.count == 1 ? "" : "s") unpriced"
    }

    private var detailLabel: String {
        " — spend below excludes \(unpriced.rows) token row\(unpriced.rows == 1 ? "" : "s") from \(unpriced.models.joined(separator: ", "))"
    }

    var body: some View {
        TintBanner(tone: Color.status(.awaiting), toneBg: Color.status(.awaiting).opacity(0.08)) {
            (
                Text(modelCountLabel)
                    .font(.noteText.weight(.semibold))
                    .foregroundStyle(Color.rupuInk)
                + Text(detailLabel)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuDim)
            )
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
