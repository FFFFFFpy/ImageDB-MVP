import { useCallback, useEffect, useRef, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type { CommitProgress, ImportPlan } from '../lib/ipc/types';

interface CommitPageProps {
  onNavigate: (route: Route) => void;
}

type Phase = 'confirm' | 'committing' | 'result';

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function isTerminalProgress(progress: CommitProgress): boolean {
  return (
    progress.state === 'completed' ||
    progress.state === 'completed_with_errors' ||
    progress.state === 'failed'
  );
}

export function CommitPage({ onNavigate }: CommitPageProps) {
  const [phase, setPhase] = useState<Phase>('confirm');
  const [plan, setPlan] = useState<ImportPlan | null>(null);
  const [progress, setProgress] = useState<CommitProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [importRunId, setImportRunId] = useState<string | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const latestRun = useQuery({
    queryKey: ['latestCompletedImportRun'],
    queryFn: api.getLatestCompletedImportRun,
  });

  const planQuery = useQuery({
    queryKey: ['commitImportPlan', latestRun.data],
    queryFn: () => api.generateImportPlan(latestRun.data!),
    enabled: !!latestRun.data && phase === 'confirm',
  });

  useEffect(() => {
    if (planQuery.data) {
      setPlan(planQuery.data);
      setImportRunId(planQuery.data.import_run_id);
    }
  }, [planQuery.data]);

  useEffect(() => {
    if (phase !== 'committing') return;

    pollRef.current = setInterval(async () => {
      try {
        const nextProgress = await api.getCommitProgress();
        setProgress(nextProgress);
        if (isTerminalProgress(nextProgress)) {
          if (pollRef.current) clearInterval(pollRef.current);
          setPhase('result');
        }
      } catch {
        // The event stream usually carries the same data; transient poll errors
        // should not interrupt a running import.
      }
    }, 1000);

    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [phase]);

  const commitMutation = useMutation({
    mutationFn: (runId: string) => api.startImportCommit(runId),
    onSuccess: () => {
      setError(null);
      setPhase('committing');
    },
    onError: (err) => {
      setError(String(err));
    },
  });

  const handleStartCommit = useCallback(() => {
    if (!importRunId) return;
    commitMutation.mutate(importRunId);
  }, [commitMutation, importRunId]);

  const handleCancel = useCallback(async () => {
    try {
      await api.cancelImportCommit();
    } catch (err) {
      setError(String(err));
    }
  }, []);

  const totalFileSize = plan
    ? plan.kept_images.reduce((sum, image) => sum + image.file_size, 0)
    : 0;

  if (phase === 'confirm') {
    return (
      <div className="commit-page">
        <h1>Import Commit</h1>

        {latestRun.isLoading && <p>Loading latest completed import run...</p>}
        {!latestRun.data && !latestRun.isLoading && (
          <div className="commit-empty">
            <p>No completed import run is ready to commit.</p>
            <button className="btn-primary" onClick={() => onNavigate('scan')}>
              Go to Scan
            </button>
          </div>
        )}

        {planQuery.isLoading && <p>Preparing import plan...</p>}
        {planQuery.isError && (
          <div className="commit-error">
            <p>{String(planQuery.error)}</p>
            <button className="btn-primary" onClick={() => onNavigate('review')}>
              Go to Review
            </button>
          </div>
        )}

        {plan && (
          <div className="commit-confirm">
            <div className="commit-stats">
              <div className="stat-card">
                <div className="stat-value">{plan.total_albums}</div>
                <div className="stat-label">Albums</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{plan.kept_images.length}</div>
                <div className="stat-label">Kept Images</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{plan.excluded_count}</div>
                <div className="stat-label">Excluded</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{formatFileSize(totalFileSize)}</div>
                <div className="stat-label">Estimated Size</div>
              </div>
            </div>

            {plan.skipped_albums.length > 0 && (
              <div className="commit-section">
                <h3>Skipped Albums</h3>
                <ul>
                  {plan.skipped_albums.map((album) => (
                    <li key={album}>{album}</li>
                  ))}
                </ul>
              </div>
            )}

            <div className="commit-section">
              <h3>Files To Commit</h3>
              <table className="commit-table">
                <thead>
                  <tr>
                    <th>Album</th>
                    <th>File</th>
                    <th>Size</th>
                  </tr>
                </thead>
                <tbody>
                  {plan.kept_images.map((image) => (
                    <tr key={image.image_id}>
                      <td>{image.album_name}</td>
                      <td className="mono">{image.relative_path}</td>
                      <td>{formatFileSize(image.file_size)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {error && <div className="commit-error-msg">{error}</div>}

            <div className="commit-actions">
              <button
                className="btn-primary"
                onClick={handleStartCommit}
                disabled={commitMutation.isPending || plan.kept_images.length === 0}
              >
                {commitMutation.isPending ? 'Starting...' : 'Commit Import'}
              </button>
            </div>
          </div>
        )}
      </div>
    );
  }

  if (phase === 'committing') {
    const pct =
      progress && progress.albums_total > 0
        ? Math.round((progress.albums_completed / progress.albums_total) * 100)
        : 0;

    return (
      <div className="commit-page">
        <h1>Committing Import</h1>
        <div className="commit-progress">
          <div className="progress-bar-container">
            <div className="progress-bar" style={{ width: `${pct}%` }} />
          </div>
          <div className="progress-text">{pct}%</div>

          <div className="progress-details">
            <div>Stage: {progress?.current_stage ?? 'preparing'}</div>
            {progress?.current_album && <div>Album: {progress.current_album}</div>}
            <div>
              Albums: {progress?.albums_completed ?? 0} / {progress?.albums_total ?? 0}
            </div>
            <div>Images committed: {progress?.images_committed ?? 0}</div>
            <div>Skipped: {progress?.albums_skipped ?? 0}</div>
            <div>Failed: {progress?.albums_failed ?? 0}</div>
          </div>

          {progress && progress.errors.length > 0 && (
            <div className="commit-errors">
              <h3>Errors</h3>
              <ul>
                {progress.errors.map((item, index) => (
                  <li key={`${index}-${item}`}>{item}</li>
                ))}
              </ul>
            </div>
          )}

          <div className="commit-actions">
            <button className="btn-danger" onClick={handleCancel}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    );
  }

  const isSuccess = progress?.state === 'completed';

  return (
    <div className="commit-page">
      <h1>Commit Result</h1>

      <div className={`commit-result ${isSuccess ? 'success' : 'partial'}`}>
        <div className="result-status">
          {isSuccess ? 'Import committed' : 'Import finished with issues'}
        </div>
        <div className="commit-stats">
          <div className="stat-card">
            <div className="stat-value">{progress?.albums_total ?? 0}</div>
            <div className="stat-label">Albums</div>
          </div>
          <div className="stat-card success">
            <div className="stat-value">
              {(progress?.albums_completed ?? 0) - (progress?.albums_skipped ?? 0) - (progress?.albums_failed ?? 0)}
            </div>
            <div className="stat-label">Committed</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">{progress?.albums_skipped ?? 0}</div>
            <div className="stat-label">Skipped</div>
          </div>
          <div className="stat-card danger">
            <div className="stat-value">{progress?.albums_failed ?? 0}</div>
            <div className="stat-label">Failed</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">{progress?.images_committed ?? 0}</div>
            <div className="stat-label">Images</div>
          </div>
        </div>
      </div>

      {progress && progress.errors.length > 0 && (
        <div className="commit-errors">
          <h3>Errors</h3>
          <ul>
            {progress.errors.map((item, index) => (
              <li key={`${index}-${item}`}>{item}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="commit-actions">
        <button className="btn-primary" onClick={() => onNavigate('dashboard')}>
          Back to Dashboard
        </button>
      </div>
    </div>
  );
}
