// Minimal ambient shims for the handful of Node builtins the test suite needs
// (workflowGraph.test.ts reads the sample workflows off disk for the
// deriveEdges equivalence test). This project deliberately has no @types/node
// installed — see CLAUDE.md's "no new npm dependency" convention — so the
// small surface actually used is declared by hand instead of pulling in the
// package. Purely a compile-time shim: `tsc -b --noEmit` needs it, Vitest's
// Node test environment resolves the real modules at runtime regardless.
declare module 'node:fs' {
  export function readFileSync(path: string, encoding: 'utf8'): string;
  export function readdirSync(path: string): string[];
}
declare module 'node:path' {
  export function resolve(...segments: string[]): string;
  export function join(...segments: string[]): string;
}
declare const __dirname: string;
