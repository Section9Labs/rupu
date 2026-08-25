import SwiftUI

/// One sortable column in a `SortableHeaderRow` — label + layout, plus the
/// `Key` that identifies it in the row's `ListSort`. `width == nil` marks
/// the ONE flexible column (it gets `.frame(maxWidth: .infinity, ...)`
/// instead of a fixed width — mirrors `ActivityTable`'s `Subject` column,
/// the only one passed `width: nil`); callers are responsible for marking
/// exactly one column this way, same as `ActivityTable` does today.
public typealias SortableColumn<Key: Hashable> = (key: Key, label: String, width: CGFloat?, alignment: Alignment)

/// The shared header-row idiom lifted from `ActivityTable`'s `header`/
/// `headerCell`/`headerCellContent`/`sortIndicator` (RupuActivity) — an
/// `Eyebrow` label + reserved-space direction chevron per sortable column,
/// generalized over any `Key` so Phase 5A's Projects/Fleet/Library screens
/// (and anything after) can reuse it instead of re-deriving the idiom.
/// `ActivityTable` itself is left as-is by this task — migrating it onto
/// this type is not part of this change.
///
/// Tapping a column header makes it the active sort key (first tap picks a
/// direction from the column's `alignment` — see `defaultAscending(for:)`
/// below) or, if it's already active, flips direction. `sort` is an
/// external binding so the owning screen's `@State` (and any live-data
/// re-sort-on-mutate behavior that follows from it, the same pattern
/// `ActivityTable` uses) stays with the caller.
public struct SortableHeaderRow<Key: Hashable & CaseIterable>: View {
    private let columns: [SortableColumn<Key>]
    @Binding private var sort: ListSort<Key>

    public init(columns: [SortableColumn<Key>], sort: Binding<ListSort<Key>>) {
        self.columns = columns
        self._sort = sort
    }

    public var body: some View {
        HStack(spacing: 0) {
            ForEach(columns, id: \.key) { column in
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

    private func headerCellContent(_ column: SortableColumn<Key>) -> some View {
        Button {
            toggleSort(column)
        } label: {
            HStack(spacing: 4) {
                if column.alignment == .trailing {
                    sortIndicator(for: column.key)
                    Eyebrow(column.label)
                } else {
                    Eyebrow(column.label)
                    sortIndicator(for: column.key)
                }
            }
        }
        .buttonStyle(.plain)
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

    private func toggleSort(_ column: SortableColumn<Key>) {
        if sort.key == column.key {
            sort.ascending.toggle()
        } else {
            sort = ListSort(key: column.key, ascending: Self.defaultAscending(for: column))
        }
    }

    /// First-tap direction for a newly-selected column. `ActivitySort.Key`
    /// hand-writes this per case (`defaultAscending`); a generic `Key` has
    /// no such per-case hook to call, so this reads the same signal
    /// `ActivityTable` already encodes in its header layout: every one of
    /// its trailing-aligned columns (`Dur`, `Cost`, `Started` — right-aligned
    /// so their monospaced digits line up) is also one of its
    /// descending-default columns, and every leading-aligned (text) column
    /// is ascending-default. Trailing alignment stands in for "numeric/date,
    /// biggest-or-most-recent-first" and non-trailing for "text, A→Z" — the
    /// same macOS-native convention (Finder/Mail date & size columns),
    /// derived from layout instead of restated per key.
    private static func defaultAscending(for column: SortableColumn<Key>) -> Bool {
        column.alignment != .trailing
    }
}
