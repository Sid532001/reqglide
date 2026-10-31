import type { KeyValueEntry, MultipartPart, MultipartValue } from "./api";
import { KeyValueEditor } from "./KeyValueEditor";
import { DEFAULT_RAW_CONTENT_TYPE } from "./request-framing";

export type BodyMode =
  | "none"
  | "raw"
  | "formUrlEncoded"
  | "multipart"
  | "binary";

interface BodyEditorProps {
  mode: BodyMode;
  rawContent: string;
  rawContentType: string;
  formFields: KeyValueEntry[];
  multipartParts: MultipartPart[];
  binaryFilePath: string;
  binaryContentType: string;
  onModeChange: (mode: BodyMode) => void;
  onRawContentChange: (content: string) => void;
  onRawContentTypeChange: (contentType: string) => void;
  onFormFieldsChange: (fields: KeyValueEntry[]) => void;
  onMultipartPartsChange: (parts: MultipartPart[]) => void;
  onBinaryFilePathChange: (path: string) => void;
  onBinaryContentTypeChange: (contentType: string) => void;
  onPickFile: () => Promise<string | null>;
}

const RAW_TYPES = [
  [DEFAULT_RAW_CONTENT_TYPE, "JSON"],
  ["text/plain", "Text"],
  ["application/xml", "XML"],
  ["text/html", "HTML"],
  ["application/javascript", "JavaScript"],
] as const;

const BODY_MODES: BodyMode[] = [
  "none",
  "raw",
  "formUrlEncoded",
  "multipart",
  "binary",
];

function bodyModeLabel(mode: BodyMode): string {
  if (mode === "formUrlEncoded") {
    return "x-www-form-urlencoded";
  }
  if (mode === "multipart") {
    return "form-data";
  }
  return mode;
}

function defaultPart(): MultipartPart {
  return {
    name: "",
    enabled: true,
    value: { kind: "text", content: "" },
  };
}

export function BodyEditor({
  mode,
  rawContent,
  rawContentType,
  formFields,
  multipartParts,
  binaryFilePath,
  binaryContentType,
  onModeChange,
  onRawContentChange,
  onRawContentTypeChange,
  onFormFieldsChange,
  onMultipartPartsChange,
  onBinaryFilePathChange,
  onBinaryContentTypeChange,
  onPickFile,
}: BodyEditorProps) {
  function updatePart(index: number, update: (part: MultipartPart) => MultipartPart) {
    onMultipartPartsChange(
      multipartParts.map((part, partIndex) =>
        partIndex === index ? update(part) : part,
      ),
    );
  }

  function updatePartValue(
    index: number,
    update: (value: MultipartValue) => MultipartValue,
  ) {
    updatePart(index, (part) => ({ ...part, value: update(part.value) }));
  }

  async function chooseMultipartFile(index: number) {
    const path = await onPickFile();
    if (path) {
      updatePartValue(index, (value) =>
        value.kind === "file" ? { ...value, path } : value,
      );
    }
  }

  async function chooseBinaryFile() {
    const path = await onPickFile();
    if (path) {
      onBinaryFilePathChange(path);
    }
  }

  return (
    <div className="body-editor-shell">
      <div className="body-mode-toolbar" role="tablist" aria-label="Request body type">
        {BODY_MODES.map((bodyMode) => (
          <button
            key={bodyMode}
            className={mode === bodyMode ? "active" : undefined}
            type="button"
            role="tab"
            aria-selected={mode === bodyMode}
            onClick={() => onModeChange(bodyMode)}
          >
            {bodyModeLabel(bodyMode)}
          </button>
        ))}
        {mode === "raw" && (
          <select
            aria-label="Raw body content type"
            value={rawContentType}
            onChange={(event) => onRawContentTypeChange(event.target.value)}
          >
            {!RAW_TYPES.some(([value]) => value === rawContentType) && (
              <option value={rawContentType}>{rawContentType || "No content type"}</option>
            )}
            {RAW_TYPES.map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
        )}
      </div>

      {mode === "none" && (
        <div className="empty-state compact">
          <strong>This request has no body</strong>
          <span>Select a body type to add text, fields, or local file data.</span>
        </div>
      )}
      {mode === "raw" && (
        <textarea
          className="body-editor"
          aria-label="Request body"
          value={rawContent}
          spellCheck={false}
          placeholder={'{\n  "message": "Hello"\n}'}
          onChange={(event) => onRawContentChange(event.target.value)}
        />
      )}
      {mode === "formUrlEncoded" && (
        <KeyValueEditor
          entries={formFields}
          nameLabel="Field"
          addLabel="Add field"
          onChange={onFormFieldsChange}
        />
      )}
      {mode === "multipart" && (
        <div className="multipart-editor">
          <div className="multipart-heading">
            <span />
            <span>Field</span>
            <span>Type</span>
            <span>Value</span>
            <span />
          </div>
          {multipartParts.map((part, index) => (
            <div className="multipart-part" key={index}>
              <div className="multipart-row">
                <input
                  aria-label={`Enable ${part.name || "multipart field"}`}
                  type="checkbox"
                  checked={part.enabled}
                  onChange={(event) =>
                    updatePart(index, (current) => ({
                      ...current,
                      enabled: event.target.checked,
                    }))
                  }
                />
                <input
                  aria-label="Multipart field name"
                  value={part.name}
                  placeholder="Field"
                  onChange={(event) =>
                    updatePart(index, (current) => ({
                      ...current,
                      name: event.target.value,
                    }))
                  }
                />
                <select
                  aria-label={`Type for ${part.name || "multipart field"}`}
                  value={part.value.kind}
                  onChange={(event) =>
                    updatePartValue(index, (value) =>
                      event.target.value === "file"
                        ? { kind: "file", path: "", contentType: value.contentType }
                        : { kind: "text", content: "", contentType: value.contentType },
                    )
                  }
                >
                  <option value="text">Text</option>
                  <option value="file">File</option>
                </select>
                {part.value.kind === "text" ? (
                  <input
                    aria-label={`Value for ${part.name || "multipart field"}`}
                    value={part.value.content}
                    placeholder="Value"
                    onChange={(event) =>
                      updatePartValue(index, (value) =>
                        value.kind === "text"
                          ? { ...value, content: event.target.value }
                          : value,
                      )
                    }
                  />
                ) : (
                  <div className="file-path-control">
                    <input
                      aria-label={`File path for ${part.name || "multipart field"}`}
                      value={part.value.path}
                      placeholder="Choose a local file"
                      onChange={(event) =>
                        updatePartValue(index, (value) =>
                          value.kind === "file"
                            ? { ...value, path: event.target.value }
                            : value,
                        )
                      }
                    />
                    <button type="button" onClick={() => void chooseMultipartFile(index)}>
                      Browse
                    </button>
                  </div>
                )}
                <button
                  type="button"
                  aria-label={`Remove ${part.name || "multipart field"}`}
                  onClick={() =>
                    onMultipartPartsChange(
                      multipartParts.filter((_, partIndex) => partIndex !== index),
                    )
                  }
                >
                  ×
                </button>
              </div>
              <div className="multipart-metadata">
                <label>
                  Content type
                  <input
                    aria-label={`Content type for ${part.name || "multipart field"}`}
                    value={part.value.contentType ?? ""}
                    placeholder={
                      part.value.kind === "file" ? "Auto-detect" : "text/plain"
                    }
                    onChange={(event) =>
                      updatePartValue(index, (value) => ({
                        ...value,
                        contentType: event.target.value || undefined,
                      }))
                    }
                  />
                </label>
                {part.value.kind === "file" && (
                  <label>
                    Sent filename
                    <input
                      aria-label={`Sent filename for ${part.name || "multipart field"}`}
                      value={part.value.fileName ?? ""}
                      placeholder="Use local filename"
                      onChange={(event) =>
                        updatePartValue(index, (value) =>
                          value.kind === "file"
                            ? { ...value, fileName: event.target.value || undefined }
                            : value,
                        )
                      }
                    />
                  </label>
                )}
              </div>
            </div>
          ))}
          <button
            className="add-row"
            type="button"
            onClick={() => onMultipartPartsChange([...multipartParts, defaultPart()])}
          >
            + Add multipart field
          </button>
          <p className="local-file-note">
            File paths stay local and are read only when you send this request.
          </p>
        </div>
      )}
      {mode === "binary" && (
        <div className="binary-editor">
          <label>
            Local file
            <div className="file-path-control">
              <input
                aria-label="Binary file path"
                value={binaryFilePath}
                placeholder="Choose a local file"
                onChange={(event) => onBinaryFilePathChange(event.target.value)}
              />
              <button type="button" onClick={() => void chooseBinaryFile()}>
                Browse
              </button>
            </div>
          </label>
          <label>
            Content type
            <input
              aria-label="Binary content type"
              value={binaryContentType}
              placeholder="application/octet-stream"
              onChange={(event) => onBinaryContentTypeChange(event.target.value)}
            />
          </label>
          <p className="local-file-note">
            The file is streamed from disk when sent. Its path is saved with the request and
            may need updating on another machine.
          </p>
        </div>
      )}
    </div>
  );
}
