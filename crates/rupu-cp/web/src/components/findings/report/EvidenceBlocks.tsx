import { useState, type ReactNode } from 'react';
import { findingArtifactUrl } from '../../../lib/api';
import { formatBytes, type ArtifactRef, type DisasmLine, type EvidenceBlock } from '../../../lib/findingReport';
import Markdown from '../../transcript/Markdown';

/** An address for display. The CP's exact `0x…` string wins; a bare number
 *  above 2^53 was already rounded by JSON.parse, so it is marked imprecise
 *  rather than shown as if it were right. */
function hex(n: number, exact?: string): string {
  if (exact) return exact;
  const s = `0x${n.toString(16)}`;
  return Number.isSafeInteger(n) ? s : `${s} ≈ (imprecise)`;
}

const PRE = 'overflow-auto px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre';

/** Scroll caps. `compact` is the Code tab's inline finding card, which sits
 *  between source lines and must stay short; the report page uses the full
 *  caps. */
const SCROLL_CAP = { full: 'max-h-96', compact: 'max-h-48' } as const;
const IMAGE_CAP = { full: 'max-h-[32rem]', compact: 'max-h-48' } as const;
type Density = keyof typeof SCROLL_CAP;

function Frame({ label, children }: { label?: ReactNode; children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-md border border-border bg-panel">
      {label && <div className="border-b border-border px-3 py-1.5 font-mono text-note text-ink-mute">{label}</div>}
      {children}
    </div>
  );
}

function Code({ text, density }: { text: string; density: Density }) {
  return <pre className={`${SCROLL_CAP[density]} ${PRE}`}>{text}</pre>;
}

/** True when rupu verified and stored this file. Before evidence-block files
 *  were ingested, agents wrote block refs verbatim: such a ref may carry an
 *  empty or malformed `sha256` and no `stored`, and the CP cannot serve it. */
function isRecorded(artifact: ArtifactRef): boolean {
  return /^[0-9a-f]{64}$/.test(artifact.sha256 ?? '') && artifact.stored !== undefined;
}

const UNRECORDED_NOTE = 'file not recorded by rupu (finding predates file verification)';

function DownloadLink({ findingId, artifact }: { findingId: string; artifact: ArtifactRef }) {
  return (
    <a href={findingArtifactUrl(findingId, artifact.sha256)} download className="text-brand-700 hover:underline">
      Download
    </a>
  );
}

function ArtifactMeta({ findingId, artifact }: { findingId: string; artifact: ArtifactRef }) {
  if (!isRecorded(artifact)) {
    return (
      <span className="flex flex-wrap items-center gap-3 font-mono text-note text-ink-mute">
        <span className="truncate">{artifact.path}</span>
        <span className="font-sans italic">{UNRECORDED_NOTE}</span>
      </span>
    );
  }
  return (
    <span className="flex flex-wrap items-center gap-3 font-mono text-note text-ink-mute">
      <span className="truncate">{artifact.path}</span>
      <span>{formatBytes(artifact.size)}</span>
      {artifact.host && <span>from host {artifact.host}</span>}
      <DownloadLink findingId={findingId} artifact={artifact} />
    </span>
  );
}

/** An image artifact. A remote one (`host` set) is pulled by the CP on first
 *  request, so it is never requested on render, only after a click. An
 *  unrecorded one is never requested at all. */
function ImageBlock({ findingId, artifact, caption, density }: { findingId: string; artifact: ArtifactRef; caption?: string; density: Density }) {
  if (!isRecorded(artifact)) {
    return (
      <figure className="overflow-hidden rounded-md border border-border bg-panel">
        {caption && <figcaption className="px-3 py-1.5 text-ui text-ink-dim">{caption}</figcaption>}
        <div className={caption ? 'border-t border-border px-3 py-1.5' : 'px-3 py-1.5'}>
          <ArtifactMeta findingId={findingId} artifact={artifact} />
        </div>
      </figure>
    );
  }
  return <RecordedImage findingId={findingId} artifact={artifact} caption={caption} density={density} />;
}

function RecordedImage({ findingId, artifact, caption, density }: { findingId: string; artifact: ArtifactRef; caption?: string; density: Density }) {
  const [show, setShow] = useState(!artifact.host);
  const [broken, setBroken] = useState(false);
  const url = findingArtifactUrl(findingId, artifact.sha256);
  return (
    <figure className="overflow-hidden rounded-md border border-border bg-panel">
      <div className="px-3 py-2">
        {broken ? (
          <p role="alert" className="text-ui text-err">This image could not be displayed. Use Download.</p>
        ) : show ? (
          <img
            src={url}
            alt={caption ?? artifact.path}
            loading="lazy"
            className={`${IMAGE_CAP[density]} max-w-full rounded border border-border`}
            onError={() => setBroken(true)}
          />
        ) : (
          <button
            type="button"
            onClick={() => setShow(true)}
            className="rounded border border-border bg-surface px-2 py-1 text-ui text-ink hover:bg-brand-50"
          >
            Load image (from host {artifact.host})
          </button>
        )}
      </div>
      {caption && <figcaption className="px-3 pb-1.5 text-ui text-ink-dim">{caption}</figcaption>}
      <div className="border-t border-border px-3 py-1.5">
        <ArtifactMeta findingId={findingId} artifact={artifact} />
      </div>
    </figure>
  );
}

function Disasm({ arch, listing, density }: { arch: string; listing: DisasmLine[]; density: Density }) {
  return (
    <Frame label={`disassembly (${arch})`}>
      <div className={`${SCROLL_CAP[density]} overflow-auto`}>
        <table className="w-full border-collapse font-mono text-note text-ink">
          <tbody>
            {listing.map((l, i) => (
              <tr key={i} className="align-top">
                <td className="whitespace-nowrap px-3 py-0.5 text-ink-mute">{hex(l.address, l.address_hex)}</td>
                <td className="whitespace-nowrap px-2 py-0.5 text-ink-mute">{l.bytes}</td>
                <td className="whitespace-nowrap px-2 py-0.5">{l.mnemonic}</td>
                <td className="px-2 py-0.5 text-ink-dim">{l.ops}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Frame>
  );
}

function Table({ headers, rows, density }: { headers: string[]; rows: string[][]; density: Density }) {
  return (
    <div className={`${density === 'compact' ? SCROLL_CAP.compact : ''} overflow-auto rounded-md border border-border bg-panel`}>
      <table className="w-full border-collapse text-ui text-ink">
        <thead>
          <tr className="border-b border-border text-left text-note text-ink-mute">
            {headers.map((h, i) => <th key={i} className="px-3 py-1.5 font-medium">{h}</th>)}
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i} className="border-b border-border last:border-0">
              {r.map((cell, j) => <td key={j} className="px-3 py-1 font-mono text-note">{cell}</td>)}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Block({ findingId, block, density }: { findingId: string; block: EvidenceBlock; density: Density }) {
  switch (block.kind) {
    case 'text':
      return <div className="text-ink-dim"><Markdown text={block.text} /></div>;
    case 'code_slice':
      return <Frame label={[block.file, block.lang].filter(Boolean).join(' · ') || undefined}><Code text={block.excerpt} density={density} /></Frame>;
    case 'diff':
      return <Frame label="diff"><Code text={block.diff} density={density} /></Frame>;
    case 'table':
      return <Table headers={block.headers} rows={block.rows} density={density} />;
    case 'image':
      return <ImageBlock findingId={findingId} artifact={block.artifact} caption={block.caption} density={density} />;
    case 'hexdump':
      return (
        <Frame label={<span className="flex flex-wrap items-center justify-between gap-2"><span>hexdump · base {hex(block.base, block.base_hex)}</span><ArtifactMeta findingId={findingId} artifact={block.artifact} /></span>}>
          {block.rendered ? (
            <Code text={block.rendered} density={density} />
          ) : (
            <p className="px-3 py-2 text-ui text-ink-mute">
              {isRecorded(block.artifact) ? 'No rendered preview. Use Download.' : 'No rendered preview.'}
            </p>
          )}
        </Frame>
      );
    case 'disasm':
      return <Disasm arch={block.arch} listing={block.listing} density={density} />;
    case 'decompile':
      return <Frame label={`decompiled (${block.lang})`}><Code text={block.listing} density={density} /></Frame>;
    case 'http_exchange':
      return (
        <Frame label="HTTP exchange">
          <div className="text-note text-ink-mute px-3 pt-1.5">Request</div>
          <Code text={block.request} density={density} />
          <div className="border-t border-border px-3 pt-1.5 text-note text-ink-mute">Response</div>
          <Code text={block.response} density={density} />
        </Frame>
      );
    case 'scan_output':
      return <Frame label={block.tool}><Code text={block.output} density={density} /></Frame>;
    case 'pcap_ref':
      return (
        <Frame label={<ArtifactMeta findingId={findingId} artifact={block.artifact} />}>
          <p className="px-3 py-2 text-ui text-ink-dim">{block.summary}</p>
        </Frame>
      );
    default: {
      // A block kind from a newer build: name it rather than drop it.
      const kind = (block as { kind?: unknown }).kind;
      return (
        <Frame>
          <p className="px-3 py-2 text-ui text-ink-mute">Unsupported evidence block: {String(kind)}</p>
        </Frame>
      );
    }
  }
}

export default function EvidenceBlocks({
  findingId,
  blocks,
  compact = false,
}: {
  findingId: string;
  blocks: EvidenceBlock[];
  /** Tighter spacing and smaller image / scroll caps, for the inline card. */
  compact?: boolean;
}) {
  const density: Density = compact ? 'compact' : 'full';
  return (
    <div className={compact ? 'space-y-2' : 'space-y-3'}>
      {blocks.map((b, i) => <Block key={i} findingId={findingId} block={b} density={density} />)}
    </div>
  );
}
