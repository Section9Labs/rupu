/** Refresh cadence for the session view: fast while a turn is in flight, slow otherwise. */
export function pollIntervalFor(
  session: { status?: unknown; active_run_id?: string | null } | null,
): number {
  if (!session) return 5000;
  const active = session.status === 'running' || !!session.active_run_id;
  return active ? 1500 : 5000;
}
