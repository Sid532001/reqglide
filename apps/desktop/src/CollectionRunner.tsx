import { useEffect, useMemo, useState } from "react";
import {
  cancelHttpRequest,
  deleteCollectionRunReport,
  exportCollectionRunReport,
  listCollectionRunReports,
  loadCollectionRunReport,
  runCollection,
  type CollectionRunReport,
  type CollectionRunReportSummary,
  type WorkspaceDocument,
} from "./api";
import { collectionRunTargets, parseRunData } from "./collection-runner";
import { formatBytes } from "./format";

interface CollectionRunnerProps {
  workspace: WorkspaceDocument;
  activeEnvironmentId: string | null;
  onClose: () => void;
  onError: (message: string) => void;
}

export function CollectionRunner({
  workspace,
  activeEnvironmentId,
  onClose,
  onError,
}: CollectionRunnerProps) {
  const targets = useMemo(() => collectionRunTargets(workspace), [workspace]);
  const [targetKey, setTargetKey] = useState(targets[0]?.key ?? "");
  const [environmentId, setEnvironmentId] = useState(activeEnvironmentId ?? "");
  const [iterations, setIterations] = useState(1);
  const [delayMs, setDelayMs] = useState(0);
  const [stopOnFailure, setStopOnFailure] = useState(true);
  const [dataSource, setDataSource] = useState("");
  const [isRunning, setIsRunning] = useState(false);
  const [report, setReport] = useState<CollectionRunReport | null>(null);
  const [reports, setReports] = useState<CollectionRunReportSummary[]>([]);
  const [isLoadingReports, setIsLoadingReports] = useState(true);

  useEffect(() => {
    let cancelled = false;
    void listCollectionRunReports(workspace.id)
      .then((result) => {
        if (!cancelled) {
          setReports(result);
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          onError(errorMessage(cause));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsLoadingReports(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [onError, workspace.id]);

  async function startRun() {
    const target = targets.find((candidate) => candidate.key === targetKey);
    if (!target || isRunning) {
      return;
    }
    let dataRows: Array<Record<string, string>>;
    try {
      dataRows = parseRunData(dataSource);
    } catch (cause) {
      onError(errorMessage(cause));
      return;
    }

    setIsRunning(true);
    try {
      const nextReport = await runCollection({
        target: target.target,
        environmentId: environmentId || null,
        iterations,
        delayMs,
        stopOnFailure,
        dataRows,
      });
      setReport(nextReport);
      setReports(await listCollectionRunReports(workspace.id));
    } catch (cause) {
      const message = errorMessage(cause);
      if (message.toLocaleLowerCase() !== "collection run cancelled") {
        onError(message);
      }
    } finally {
      setIsRunning(false);
    }
  }

  async function cancelRun() {
    try {
      await cancelHttpRequest();
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function openReport(reportId: string) {
    try {
      setReport(await loadCollectionRunReport(workspace.id, reportId));
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function removeReport(reportId: string) {
    if (!window.confirm("Delete this local collection run report?")) {
      return;
    }
    try {
      await deleteCollectionRunReport(workspace.id, reportId);
      setReports((current) => current.filter((candidate) => candidate.id !== reportId));
      if (report?.id === reportId) {
        setReport(null);
      }
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function exportReport(format: "json" | "junit") {
    if (!report) {
      return;
    }
    try {
      await exportCollectionRunReport(workspace.id, report.id, format);
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  const parsedDataCount = useMemo(() => {
    try {
      return parseRunData(dataSource).length;
    } catch {
      return null;
    }
  }, [dataSource]);

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className="runner-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="runner-title"
      >
        <header className="runner-heading">
          <div>
            <strong id="runner-title">Collection runner</strong>
            <span>Run saved requests sequentially with shared script state and local reports.</span>
          </div>
          <button type="button" aria-label="Close collection runner" disabled={isRunning} onClick={onClose}>
            ×
          </button>
        </header>

        <div className="runner-layout">
          <aside className="runner-config">
            <label className="network-field">
              <span>Collection or folder</span>
              <select value={targetKey} onChange={(event) => setTargetKey(event.target.value)}>
                {targets.map((target) => (
                  <option key={target.key} value={target.key} disabled={target.requestCount === 0}>
                    {target.label} ({target.requestCount})
                  </option>
                ))}
              </select>
            </label>
            <label className="network-field">
              <span>Environment</span>
              <select
                value={environmentId}
                onChange={(event) => setEnvironmentId(event.target.value)}
              >
                <option value="">None</option>
                {workspace.environments.map((environment) => (
                  <option key={environment.id} value={environment.id}>
                    {environment.name}
                  </option>
                ))}
              </select>
            </label>
            <div className="runner-number-grid">
              <label className="network-field">
                <span>Iterations</span>
                <input
                  type="number"
                  min={1}
                  max={1000}
                  value={iterations}
                  onChange={(event) => setIterations(clampNumber(event.target.value, 1, 1000))}
                />
              </label>
              <label className="network-field">
                <span>Delay (ms)</span>
                <input
                  type="number"
                  min={0}
                  max={60_000}
                  value={delayMs}
                  onChange={(event) => setDelayMs(clampNumber(event.target.value, 0, 60_000))}
                />
              </label>
            </div>
            <label className="network-check">
              <input
                type="checkbox"
                checked={stopOnFailure}
                onChange={(event) => setStopOnFailure(event.target.checked)}
              />
              <span>
                <strong>Stop on first failure</strong>
                <small>A failed script test or request error stops the run.</small>
              </span>
            </label>
            <label className="runner-data-field">
              <span>Data rows (JSON or CSV)</span>
              <textarea
                spellCheck={false}
                value={dataSource}
                placeholder={'[{"userId":"1"}, {"userId":"2"}]\n\nor\n\nuserId,name\n1,Ada'}
                onChange={(event) => setDataSource(event.target.value)}
              />
              <small>
                {parsedDataCount === null
                  ? "Data has an error."
                  : parsedDataCount > 0
                    ? `${parsedDataCount} data row${parsedDataCount === 1 ? "" : "s"}; row count replaces iterations.`
                    : "Each row becomes local variables for one iteration."}
              </small>
            </label>
            <div className="runner-actions">
              {isRunning ? (
                <button className="danger-button" type="button" onClick={() => void cancelRun()}>
                  Cancel run
                </button>
              ) : (
                <button
                  className="primary-small"
                  type="button"
                  disabled={!targetKey || parsedDataCount === null}
                  onClick={() => void startRun()}
                >
                  Run
                </button>
              )}
            </div>

            <div className="runner-report-list">
              <strong>Local reports</strong>
              {isLoadingReports ? (
                <span>Loading…</span>
              ) : reports.length ? (
                reports.map((savedReport) => (
                  <div
                    className={`runner-report-item ${report?.id === savedReport.id ? "active" : ""}`}
                    key={savedReport.id}
                  >
                    <button type="button" onClick={() => void openReport(savedReport.id)}>
                      <strong>{savedReport.targetName}</strong>
                      <span>{new Date(savedReport.startedAtMs).toLocaleString()}</span>
                      <small>
                        {savedReport.summary.requestsExecuted} requests · {savedReport.summary.requestsFailed} failed
                      </small>
                    </button>
                    <button
                      type="button"
                      aria-label={`Delete report for ${savedReport.targetName}`}
                      onClick={() => void removeReport(savedReport.id)}
                    >
                      ×
                    </button>
                  </div>
                ))
              ) : (
                <span>No collection runs yet.</span>
              )}
            </div>
          </aside>

          <div className="runner-results">
            {isRunning ? (
              <div className="runner-empty">
                <strong>Run in progress…</strong>
                <span>Requests execute sequentially in the native engine.</span>
              </div>
            ) : report ? (
              <RunReportView report={report} onExport={exportReport} />
            ) : (
              <div className="runner-empty">
                <strong>Ready to run</strong>
                <span>Response bodies are not retained in reports; test output and metadata are.</span>
              </div>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}

function RunReportView({
  report,
  onExport,
}: {
  report: CollectionRunReport;
  onExport: (format: "json" | "junit") => Promise<void>;
}) {
  return (
    <>
      <div className="runner-summary">
        <div><strong>{report.summary.requestsExecuted}</strong><span>Requests</span></div>
        <div className={report.summary.requestsFailed ? "failed" : "passed"}>
          <strong>{report.summary.requestsFailed}</strong><span>Failed</span>
        </div>
        <div className="passed"><strong>{report.summary.testsPassed}</strong><span>Tests passed</span></div>
        <div className={report.summary.testsFailed ? "failed" : ""}>
          <strong>{report.summary.testsFailed}</strong><span>Tests failed</span>
        </div>
      </div>
      <div className="runner-report-meta">
        <div>
          <strong>{report.targetName}</strong>
          <span>
            {report.completedIterations}/{report.requestedIterations} iterations · {report.durationMs} ms · {formatBytes(report.summary.bytesReceived)}
          </span>
        </div>
        <button type="button" onClick={() => void onExport("json")}>Export JSON</button>
        <button type="button" onClick={() => void onExport("junit")}>Export JUnit</button>
      </div>
      <div className="runner-result-table">
        <div className="runner-result-row runner-result-header">
          <span>Iteration</span><span>Request</span><span>Status</span><span>Time</span><span>Tests</span>
        </div>
        {report.results.map((result, index) => (
          <details
            className={`runner-result-detail ${result.error || result.testsFailed ? "failed" : "passed"}`}
            key={`${result.iteration}:${result.requestId}:${index}`}
          >
            <summary className="runner-result-row">
              <span>{result.iteration}</span>
              <strong>{result.requestName}</strong>
              <span>{result.status ?? "ERR"}</span>
              <span>{result.durationMs} ms</span>
              <span>{result.testsPassed}/{result.testsPassed + result.testsFailed}</span>
            </summary>
            <div className="runner-result-details">
              {result.error && <p className="runner-error">{result.error}</p>}
              {result.scriptRuns.flatMap((run) =>
                run.tests.map((test) => (
                  <div className={`runner-test ${test.passed ? "passed" : "failed"}`} key={`${run.phase}:${test.name}`}>
                    <span>{test.passed ? "✓" : "×"}</span>
                    <strong>{test.name}</strong>
                    {test.error && <small>{test.error}</small>}
                  </div>
                )),
              )}
            </div>
          </details>
        ))}
      </div>
    </>
  );
}

function clampNumber(value: string, minimum: number, maximum: number): number {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) ? Math.min(maximum, Math.max(minimum, parsed)) : minimum;
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
