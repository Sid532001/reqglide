import { useEffect, useState } from "react";
import { exportCodeForEnvironment, type CodeLanguage, type RequestDefinition } from "./api";

interface CodeSnippetModalProps {
  request: RequestDefinition;
  activeEnvironmentId: string | null;
  onClose: () => void;
}

const LANGUAGES: { value: CodeLanguage; label: string }[] = [
  { value: "javaScriptFetch", label: "JavaScript – Fetch" },
  { value: "nodeAxios", label: "Node.js – Axios" },
  { value: "pythonRequests", label: "Python – Requests" },
];

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export function CodeSnippetModal({ request, activeEnvironmentId, onClose }: CodeSnippetModalProps) {
  const [language, setLanguage] = useState<CodeLanguage>("javaScriptFetch");
  const [snippet, setSnippet] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    setError(null);
    setCopied(false);
    exportCodeForEnvironment(request, activeEnvironmentId, language)
      .then((result) => {
        if (!cancelled) {
          setSnippet(result);
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setSnippet("");
          setError(errorMessage(cause));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [request, activeEnvironmentId, language]);

  async function copy() {
    try {
      await navigator.clipboard.writeText(snippet);
      setCopied(true);
    } catch {
      setError("The snippet could not be copied to the clipboard.");
    }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className="code-snippet-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="code-snippet-title"
      >
        <header className="cookie-heading">
          <div>
            <strong id="code-snippet-title">Generate code</strong>
            <span>Variables are resolved into literals — review the snippet before sharing it.</span>
          </div>
          <button type="button" aria-label="Close code generator" onClick={onClose}>
            ×
          </button>
        </header>

        <div className="code-snippet-languages" role="tablist" aria-label="Snippet language">
          {LANGUAGES.map((entry) => (
            <button
              key={entry.value}
              className={language === entry.value ? "active" : undefined}
              type="button"
              role="tab"
              aria-selected={language === entry.value}
              onClick={() => setLanguage(entry.value)}
            >
              {entry.label}
            </button>
          ))}
        </div>

        {error ? (
          <div className="code-snippet-error">{error}</div>
        ) : (
          <pre className="code-snippet-body">{isLoading ? "Generating…" : snippet}</pre>
        )}

        <footer className="modal-actions">
          <button
            className="ghost-button"
            type="button"
            disabled={!snippet || isLoading}
            onClick={() => void copy()}
          >
            {copied ? "Copied ✓" : "Copy"}
          </button>
          <button className="primary-small" type="button" onClick={onClose}>
            Done
          </button>
        </footer>
      </section>
    </div>
  );
}
