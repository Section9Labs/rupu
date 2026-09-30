// Re-export: the identity extraction lives in `lib/` so the Live Events card
// builder (`lib/situationRoom/cards.ts`) can share it without importing UI.
export { findingCodename, type FindingIdentity } from '../../lib/findingIdentity';
