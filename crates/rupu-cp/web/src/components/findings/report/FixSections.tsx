import DiffView from '../../transcript/DiffView';
import Markdown from '../../transcript/Markdown';
import CommandBlock from './CommandBlock';
import Section from './Section';
import { isSentinel, sentinelLabel, type FindingReport } from '../../../lib/findingReport';

function SentinelNote({ value }: { value: string }) {
  return <p className="text-ui text-ink-mute">{sentinelLabel(value)}</p>;
}

export default function FixSections({ report }: { report: FindingReport }) {
  const patch = report.recommended_patch;
  const ci = report.ci_cd_detection;
  const reg = report.regression_test;
  return (
    <>
      <Section id="s-patch" title="Recommended patch">
        {isSentinel(patch) ? <SentinelNote value={patch} /> : (
          <div className="space-y-2">
            <div className="overflow-hidden rounded-md border border-border"><DiffView diff={patch.diff} /></div>
            {patch.notes && <div className="text-ink-dim"><Markdown text={patch.notes} /></div>}
          </div>
        )}
      </Section>
      <Section id="s-ci" title="CI/CD detection" hint={isSentinel(ci) ? undefined : `stage: ${ci.stage}`}>
        {isSentinel(ci) ? <SentinelNote value={ci} /> : (
          <div className="space-y-2">
            <div className="text-ink-dim"><Markdown text={ci.body} /></div>
            {ci.command && <CommandBlock label="Command" command={ci.command} />}
            <p className="text-ui text-ink-dim"><span className="font-semibold text-ink">Fails when:</span> {ci.expect}</p>
          </div>
        )}
      </Section>
      <Section id="s-reg" title="Regression test">
        {isSentinel(reg) ? <SentinelNote value={reg} /> : (
          <div className="space-y-2">
            <div className="text-ink-dim"><Markdown text={reg.body} /></div>
            <CommandBlock label="Command" command={reg.command} />
            <div className="grid gap-2 sm:grid-cols-2">
              <div className="rounded-md bg-err-bg px-3 py-2 text-ui text-ink-dim">
                <div className="text-meta font-semibold uppercase tracking-wide text-err">Vulnerable build</div>{reg.expect_vulnerable}
              </div>
              <div className="rounded-md bg-ok-bg px-3 py-2 text-ui text-ink-dim">
                <div className="text-meta font-semibold uppercase tracking-wide text-ok">Patched build</div>{reg.expect_patched}
              </div>
            </div>
          </div>
        )}
      </Section>
    </>
  );
}
