import { useState, type ReactNode } from 'react';
import { findingArtifactUrl } from '../../../lib/api';
import { formatBytes, type ArtifactRef, type DisasmLine, type EvidenceBlock } from '../../../lib/findingReport';
import Markdown from '../../transcript/Markdown';

const hex = (n: number) => `0x${n.toString(16)}`;

const PRE = 'max-h-96 overflow-auto px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre';

function Frame({ label, children }: { label?: ReactNode; children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-md border border-border bg-panel">
      {label && <div className="border-b border-border px-3 py-1.5 font-mono text-note text-ink-mute">{label}</div>}
      {children}
    </div>
  );
}

function Code({ text }: { text: string }) {
  return <pre className={PRE}>{text}</pre>;
}

function DownloadLink({ findingId, artifact }: { findingId: string; artifact: ArtifactRef }) {
  return (
    <a href={findingArtifactUrl(findingId, artifact.sha256)} download className="text-brand-700 hover:underline">
      Download
    </a>
  );
}

function ArtifactMeta({ findingId, artifact }: { findingId: string; artifact: ArtifactRef }) {
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
 *  request, so it is never requested on render, only after a click. */
function ImageBlock({ findingId, artifact, caption }: { findingId: string; artifact: ArtifactRef; caption?: string }) {
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
            className="max-h-[32rem] max-w-full rounded border border-border"
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

function Disasm({ arch, listing }: { arch: string; listing: DisasmLine[] }) {
  return (
    <Frame label={`disassembly (${arch})`}>
      <div className="max-h-96 overflow-auto">
        <table className="w-full border-collapse font-mono text-note text-ink">
          <tbody>
            {listing.map((l, i) => (
              <tr key={i} className="align-top">
                <td className="whitespace-nowrap px-3 py-0.5 text-ink-mute">{hex(l.address)}</td>
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

function Table({ headers, rows }: { headers: string[]; rows: string[][] }) {
  return (
    <div className="overflow-auto rounded-md border border-border bg-panel">
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

function Block({ findingId, block }: { findingId: string; block: EvidenceBlock }) {
  switch (block.kind) {
    case 'text':
      return <div className="text-ink-dim"><Markdown text={block.text} /></div>;
    case 'code_slice':
      return <Frame label={[block.file, block.lang].filter(Boolean).join(' · ') || undefined}><Code text={block.excerpt} /></Frame>;
    case 'diff':
      return <Frame label="diff"><Code text={block.diff} /></Frame>;
    case 'table':
      return <Table headers={block.headers} rows={block.rows} />;
    case 'image':
      return <ImageBlock findingId={findingId} artifact={block.artifact} caption={block.caption} />;
    case 'hexdump':
      return (
        <Frame label={<span className="flex flex-wrap items-center justify-between gap-2"><span>hexdump · base {hex(block.base)}</span><ArtifactMeta findingId={findingId} artifact={block.artifact} /></span>}>
          {block.rendered ? <Code text={block.rendered} /> : <p className="px-3 py-2 text-ui text-ink-mute">No rendered preview. Use Download.</p>}
        </Frame>
      );
    case 'disasm':
      return <Disasm arch={block.arch} listing={block.listing} />;
    case 'decompile':
      return <Frame label={`decompiled (${block.lang})`}><Code text={block.listing} /></Frame>;
    case 'http_exchange':
      return (
        <Frame label="HTTP exchange">
          <div className="text-note text-ink-mute px-3 pt-1.5">Request</div>
          <Code text={block.request} />
          <div className="border-t border-border px-3 pt-1.5 text-note text-ink-mute">Response</div>
          <Code text={block.response} />
        </Frame>
      );
    case 'scan_output':
      return <Frame label={block.tool}><Code text={block.output} /></Frame>;
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

export default function EvidenceBlocks({ findingId, blocks }: { findingId: string; blocks: EvidenceBlock[] }) {
  return (
    <div className="space-y-3">
      {blocks.map((b, i) => <Block key={i} findingId={findingId} block={b} />)}
    </div>
  );
}
