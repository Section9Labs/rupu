import SwiftUI

/// One column in a `SortableHeaderRow` — label + layout, plus (optionally)
/// the `Key` that identifies it in the row's `ListSort`.
///
/// A struct with a memberwise-ish init rather than a 6-field tuple: once
/// `key` went optional and `firstTapAscending` was added, a positional tuple
/// stopped being legible at call sites (`(nil, "Actions", 52, .trailing,
/// nil)` reads nothing like what it means) — named arguments do.
///
/// - `width == nil` marks the ONE flexible column (it gets
///   `.frame(maxWidth: .infinity, ...)` instead of a fixed width — mirrors
///   `ActivityTable`'s `Subject` column, the only one passed `width: nil`);
///   callers are responsible for marking exactly one column this way, same
///   as `ActivityTable` does today.
/// - `key == nil` marks a plain, non-sortable column: it renders as a bare
///   `Eyebrow` label with no button and no chevron — matching
///   `ActivityTable`'s blank trailing Actions header precedent (its
///   `headerCell("", width: Layout.actions, ...)` with no `key:` argument).
/// - `firstTapAscending` is the first-tap direction override. When `nil`
///   (the default), `SortableHeaderRow` falls back to the alignment
///   heuristic documented on `SortableHeaderRow.defaultAscending(for:)`:
///   trailing-aligned columns (numeric/date, right-aligned so their
///   monospaced digits line up) default descending, everything else
///   defaults ascending. An explicit `firstTapAscending` always wins over
///   that heuristic — for a column whose alignment doesn't happen to match
///   its data's natural default direction.
public struct SortableColumn<Key: Hashable> {
    public var key: Key?
    public var label: String
    public var width: CGFloat?
    public var alignment: Alignment
    public var firstTapAscending: Bool?

    public init(
        key: Key?, label: String, width: CGFloat? = nil, alignment: Alignment = .leading,
        firstTapAscending: Bool? = nil
    ) {
        self.key = key
        self.label = label
        self.width = width
        self.alignment = alignment
        self.firstTapAscending = firstTapAscending
    }
}

/// The shared header-row idiom lifted from `ActivityTable`'s `header`/
/// `headerCell`/`headerCellContent`/`sortIndicator` (RupuActivity) — an
/// `Eyebrow` label + reserved-space direction chevron per sortable column,
/// generalized over any `Key` so Phase 5A's Projects/Fleet/Library screens
/// (and anything after) can reuse it instead of re-deriving the idiom.
/// `ActivityTable` itself is left as-is by this task — migrating it onto
/// this type is not part of this change.
///
/// Tapping a sortable column header (`key != nil`) makes it the active sort
/// key (first tap picks a direction — see `defaultAscending(for:)` below) or,
/// if it's already active, flips direction. A `key == nil` column (e.g. a
/// trailing actions column) renders its label only, inert. `sort` is an
/// external binding so the owning screen's `@State` (and any live-data
/// re-sort-on-mutate behavior that follows from it, the same pattern
/// `ActivityTable` uses) stays with the caller.
public struct SortableHeaderRow<Key: Hashable & CaseIterable & Sendable>: View {
    private let columns: [SortableColumn<Key>]
    @Binding private var sort: ListSort<Key>

    public init(columns: [SortableColumn<Key>], sort: Binding<ListSort<Key>>) {
        self.columns = columns
        self._sort = sort
    }

    public var body: some View {
        HStack(spacing: 0) {
            // Indexed rather than keyed by `column.key`: `key` is now
            // optional and more than one column (e.g. multiple non-
            // sortable columns) can legitimately be `nil`, so the key
            // alone isn't a safe `ForEach` identity.
            ForEach(Array(columns.enumerated()), id: \.offset) { _, column in
                headerCell(column)
            }
        }
    }

    @ViewBuilder
    private func headerCell(_ column: SortableColumn<Key>) -> some View {
        if let width = column.width {
            headerCellContent(column)
                .frame(width: width, alignment: column.alignment)
                .padding(.trailing, 8)
        } else {
            headerCellContent(column)
                .frame(maxWidth: .infinity, alignment: column.alignment)
                .padding(.trailing, 8)
        }
    }

    @ViewBuilder
    private func headerCellContent(_ column: SortableColumn<Key>) -> some View {
        if let key = column.key {
            Button {
                toggleSort(column, key: key)
            } label: {
                HStack(spacing: 4) {
                    if column.alignment == .trailing {
                        sortIndicator(for: key)
                        Eyebrow(column.label)
                    } else {
                        Eyebrow(column.label)
                        sortIndicator(for: key)
                    }
                }
            }
            .buttonStyle(.plain)
        } else {
            Eyebrow(column.label)
        }
    }

    /// The active header's direction chevron — space is always reserved (an
    /// invisible chevron on every other sortable header) so toggling the
    /// active column never jiggles the header row's layout. Identical to
    /// `ActivityTable.sortIndicator(for:)`.
    private func sortIndicator(for key: Key) -> some View {
        Icon(sort.key == key && !sort.ascending ? .chevronDown : .chevronUp, size: 9)
            .foregroundStyle(Color.rupuDim)
            .opacity(sort.key == key ? 1 : 0)
    }

    private func toggleSort(_ column: SortableColumn<Key>, key: Key) {
        if sort.key == key {
            sort.ascending.toggle()
        } else {
            sort = ListSort(key: key, ascending: Self.defaultAscending(for: column))
        }
    }

    /// First-tap direction for a newly-selected column. `ActivitySort.Key`
    /// hand-writes this per case (`defaultAscending`); a generic `Key` has
    /// no such per-case hook to call, so the fallback reads the same signal
    /// `ActivityTable` already encodes in its header layout: every one of
    /// its trailing-aligned columns (`Dur`, `Cost`, `Started` — right-aligned
    /// so their monospaced digits line up) is also one of its
    /// descending-default columns, and every leading-aligned (text) column
    /// is ascending-default. Trailing alignment stands in for "numeric/date,
    /// biggest-or-most-recent-first" and non-trailing for "text, A→Z" — the
    /// same macOS-native convention (Finder/Mail date & size columns),
    /// derived from layout instead of restated per key.
    ///
    /// `column.firstTapAscending`, when set, always wins over this
    /// heuristic — for the column whose alignment doesn't happen to match
    /// its data's natural default direction.
    private static func defaultAscending(for column: SortableColumn<Key>) -> Bool {
        column.firstTapAscending ?? (column.alignment != .trailing)
    }
}
