// Small fuzzy matcher (ported from Okesu's command palette): exact-substring
// beats subsequence; subsequence rewards matches at word boundaries. Returns
// the matched char indices in `text` for highlighting, or null on no match.
const BOUNDARY = /[\s\-/_.:]/;

export function fuzzyScore(
  query: string,
  text: string,
): { score: number; matched: number[] } | null {
  if (query === '') return { score: 0, matched: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();

  // Exact substring.
  const idx = t.indexOf(q);
  if (idx >= 0) {
    const matched = Array.from({ length: q.length }, (_, i) => idx + i);
    let score = 1000 + (idx === 0 ? 200 : 0);
    if (idx > 0 && BOUNDARY.test(t[idx - 1])) score += 5;
    return { score, matched };
  }

  // Subsequence walk.
  let score = 0;
  let qi = 0;
  const matched: number[] = [];
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      score += 10;
      if (ti === 0 || BOUNDARY.test(t[ti - 1])) score += 5;
      matched.push(ti);
      qi++;
    }
  }
  if (qi < q.length) return null;
  return { score, matched };
}
