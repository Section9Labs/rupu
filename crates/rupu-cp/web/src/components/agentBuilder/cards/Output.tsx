// Output card — `outputFormat` (text/json) plus, only when json, a nested
// `outputSchema` (SchemaProp[]) builder. `serializeAgent` turns the flat
// SchemaProp list into a proper JSON Schema object (required + properties);
// this card only ever edits the flat list, never the JSON Schema shape.
import { OUTPUT_FORMATS, type SchemaProp } from '../../../lib/agentBuilder/agentSpec';
import { LabeledRow, Segmented } from '../controls';
import type { CardProps } from './types';

const FORMAT_OPTIONS = OUTPUT_FORMATS.map((v) => ({ label: v, value: v as string }));
const PROP_TYPES: SchemaProp['type'][] = ['string', 'number', 'boolean', 'enum', 'array', 'object'];

export default function Output({ draft, patch }: CardProps) {
  const schema = draft.outputSchema ?? [];

  function setSchema(next: SchemaProp[]) {
    patch({ outputSchema: next });
  }
  function addProp() {
    setSchema([...schema, { name: '', type: 'string' }]);
  }
  function removeProp(i: number) {
    setSchema(schema.filter((_, idx) => idx !== i));
  }
  function updateProp(i: number, next: Partial<SchemaProp>) {
    setSchema(schema.map((p, idx) => (idx === i ? { ...p, ...next } : p)));
  }

  return (
    <>
      <LabeledRow label="Output format" yamlKey="outputFormat">
        <Segmented<string>
          options={FORMAT_OPTIONS}
          value={draft.outputFormat ?? 'text'}
          onChange={(v) => patch({ outputFormat: v === 'text' ? undefined : v })}
        />
      </LabeledRow>
      {draft.outputFormat === 'json' ? (
        <div className="ab-sub">
          <label className="ab-lab">Output schema (properties) — outputSchema</label>
          {schema.map((p, i) => (
            <div key={i} className="ab-row" style={{ gap: 6 }}>
              <div className="ab-prop">
                <input
                  className="ab-txt mono"
                  placeholder="property"
                  value={p.name}
                  onChange={(e) => updateProp(i, { name: e.target.value })}
                  aria-label={`schema property ${i} name`}
                />
                <select
                  className="ab-sel"
                  value={p.type}
                  onChange={(e) => updateProp(i, { type: e.target.value as SchemaProp['type'] })}
                  aria-label={`schema property ${i} type`}
                >
                  {PROP_TYPES.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
                <button
                  type="button"
                  className="ab-iconbtn"
                  aria-label={`remove schema property ${i}`}
                  onClick={() => removeProp(i)}
                >
                  ✕
                </button>
              </div>
              {p.type === 'enum' && (
                <input
                  className="ab-txt mono"
                  placeholder="comma,separated,values"
                  value={(p.enumValues ?? []).join(',')}
                  onChange={(e) =>
                    updateProp(i, {
                      enumValues: e.target.value
                        .split(',')
                        .map((s) => s.trim())
                        .filter(Boolean),
                    })
                  }
                  aria-label={`schema property ${i} enum values`}
                />
              )}
            </div>
          ))}
          <button type="button" className="ab-tinybtn" onClick={addProp}>
            + property
          </button>
        </div>
      ) : (
        <div className="ab-hint">
          Prompt-driven JSON only unless an outputSchema is declared. Switch to json to build a schema.
        </div>
      )}
    </>
  );
}
