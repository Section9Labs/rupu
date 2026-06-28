/**
 * Canonical id truncation for the rupu CP UI. Run / session ids are ULIDs (26
 * chars); we show the leading `head` characters plus an ellipsis. Short ids
 * (already ≤ head) are returned unchanged.
 */
export function shortId(id: string, head = 8): string {
  return id.length > head ? `${id.slice(0, head)}…` : id;
}
