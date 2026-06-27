/** Returns true while a turn is in flight (status=running or an active_run_id is set). */
export function isSessionActive(
  session: { status?: unknown; active_run_id?: string | null } | null,
): boolean {
  return session != null && (session.status === 'running' || !!session.active_run_id);
}

/** Refresh cadence for the session view: fast while a turn is in flight, slow otherwise. */
export function pollIntervalFor(
  session: { status?: unknown; active_run_id?: string | null } | null,
): number {
  return isSessionActive(session) ? 1500 : 5000;
}
