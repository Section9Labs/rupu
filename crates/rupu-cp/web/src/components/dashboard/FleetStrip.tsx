// FleetStrip — the inventory band beneath the ops blocks (spec §1).
//
// The Dashboard is an ops monitor; this row is supporting context, not a
// second subject. It therefore renders dim by default and takes weight ONLY
// where something is actually wrong — today that is exclusively "a configured
// provider failed its last probe". A count is not an alarm.
//
// The `null` discipline is the whole point of the component: a count this host
// does not report renders as an em-dash. Rendering it as `0` would make an
// unconfigured SCM connector look like an operator with no repos, and a
// never-probed provider look healthy.
//
// Link targets are the pages that already own each number. Repos and issues
// point at /settings because neither has a CP list page yet; if a /repos page
// is ever built, those two hrefs move — that is a link change, not a contract
// change.

import { Link } from 'react-router-dom';
import type { FleetCounts } from '../../lib/api';

/** Em-dash for "not reported"; the number itself for a genuine value (incl. 0). */
function num(v: number | null): string {
  return v === null || v === undefined ? '—' : String(v);
}

function Segment({
  testId,
  to,
  fault = false,
  children,
}: {
  testId: string;
  to: string;
  fault?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Link
      to={to}
      data-testid={testId}
      data-fault={fault ? 'true' : 'false'}
      className={fault ? 'text-status-failed hover:underline' : 'text-ink-mute hover:text-ink'}
    >
      {children}
    </Link>
  );
}

export function FleetStrip({
  fleet,
  fleetPartial,
}: {
  fleet: FleetCounts;
  /** True = these numbers are a partial sum, not a fleet total. */
  fleetPartial: boolean;
}) {
  // `providers_unhealthy === null` means no probe has run (no `cp serve`, or
  // the probe cache has not filled yet). That is an absence of information,
  // not a clean bill of health — so it must not weight the segment and must
  // not render an "unhealthy" clause.
  const unhealthy = fleet.providers_unhealthy;
  const providersFault = unhealthy !== null && unhealthy !== undefined && unhealthy > 0;

  const disabled = fleet.autoflows_disabled;
  const showDisabled = disabled !== null && disabled !== undefined && disabled > 0;

  const open = fleet.issues_open;
  const showOpen = open !== null && open !== undefined;

  return (
    <div
      data-testid="fleet-strip"
      className="flex flex-wrap items-center gap-x-2 gap-y-1 border-t border-border pt-2 text-xs text-ink-mute"
    >
      <Segment testId="fleet-repos" to="/settings">
        {num(fleet.repos)} repos
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-providers" to="/settings" fault={providersFault}>
        {num(fleet.providers_configured)} providers
        {providersFault && ` (${unhealthy} unhealthy)`}
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-autoflows" to="/autoflows">
        {num(fleet.autoflows_enabled)} autoflows
        {showDisabled && ` (${disabled} off)`}
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-workers" to="/workers">
        {num(fleet.workers)} workers
      </Segment>
      <span aria-hidden>·</span>
      {/* Claims live under the Claims tab on the autoflow-runs page — there is
          no dedicated /autoflows/claims route. */}
      <Segment testId="fleet-claims" to="/runs/autoflows">
        {num(fleet.claims_active)} claimed
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-issues" to="/settings">
        {num(fleet.issues_pending)} pending
        {showOpen && ` (${open}${fleet.issues_capped ? '+' : ''} open)`}
      </Segment>
      {fleetPartial && (
        <span
          className="text-ink-dim"
          title="Some reporting hosts do not supply these counts — this is a partial sum, not a fleet total."
        >
          (partial)
        </span>
      )}
    </div>
  );
}
