import type { KeyValueEntry } from "./api";

interface KeyValueEditorProps {
  entries: KeyValueEntry[];
  nameLabel: string;
  valueLabel?: string;
  addLabel: string;
  onChange: (entries: KeyValueEntry[]) => void;
}

export function KeyValueEditor({
  entries,
  nameLabel,
  valueLabel = "Value",
  addLabel,
  onChange,
}: KeyValueEditorProps) {
  function update(index: number, patch: Partial<KeyValueEntry>) {
    onChange(
      entries.map((entry, entryIndex) =>
        entryIndex === index ? { ...entry, ...patch } : entry,
      ),
    );
  }

  return (
    <div className="key-value-table">
      <div className="table-heading">
        <span />
        <span>{nameLabel}</span>
        <span>{valueLabel}</span>
        <span />
      </div>
      {entries.map((entry, index) => (
        <div className="table-row" key={index}>
          <input
            aria-label={`Enable ${entry.name || nameLabel}`}
            type="checkbox"
            checked={entry.enabled}
            onChange={(event) => update(index, { enabled: event.target.checked })}
          />
          <input
            aria-label={nameLabel}
            value={entry.name}
            placeholder={nameLabel}
            onChange={(event) => update(index, { name: event.target.value })}
          />
          <input
            aria-label={valueLabel}
            value={entry.value}
            placeholder={valueLabel}
            onChange={(event) => update(index, { value: event.target.value })}
          />
          <button
            type="button"
            aria-label={`Remove ${entry.name || nameLabel}`}
            onClick={() => onChange(entries.filter((_, entryIndex) => entryIndex !== index))}
          >
            ×
          </button>
        </div>
      ))}
      <button
        className="add-row"
        type="button"
        onClick={() => onChange([...entries, { name: "", value: "", enabled: true }])}
      >
        + {addLabel}
      </button>
    </div>
  );
}
