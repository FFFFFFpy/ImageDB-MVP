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
  // Phase 2: the persisted DB state is the single source of truth. The
  // only terminal states are the reconciler's outputs — no
  // `completed_with_errors` / `cancelled_pending_recovery` overlay.
  return (
    progress.state === 'completed' ||
    progress.state === 'failed' ||
    progress.state === 'recovery_required' ||
    progress.state === 'cancelled'
  );
}

function stageLabel(stage: string | undefined): string {
  if (!stage) return '准备中';
  const map: Record<string, string> = {
    preparing: '准备事务',
    committing: '复制到暂存区',
    processing_album: '处理图集',
    verifying: '验证暂存区',
    verified: '已验证',
    publishing: '发布目录',
    published: '已发布',
    db_committing: '数据库确认',
    library_committed: '已正式入库',
    source_archiving: '源图集归档',
    source_archived: '已完成',
    done: '完成',
    failed: '失败',
    conflict: '发生冲突',
  };
  return map[stage] ?? stage;
}

export function CommitPage({ onNavigate }: CommitPageProps) {
  const [phase, setPhase] = useState<Phase>('confirm');
  const [plan, setPlan] = useState<ImportPlan | null>(null);
  const [progress, setProgress] = useState<CommitProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [importRunId, setImportRunId] = useState<string | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Use the committable query (P0): picks up `completed`, `ready_to_commit`,
  // and `cancelled` runs with a frozen plan, so a run cancelled before any
  // transaction was prewritten can be re-committed from the same frozen plan
  // instead of getting stuck at `recovery_required` with no transaction to
  // recover.
  const latestRun = useQuery({
    queryKey: ['latestCommittableImportRun'],
    queryFn: api.getLatestCommittableImportRun,
  });

  const planQuery = useQuery({
    queryKey: ['frozenImportPlanSummary', latestRun.data],
    queryFn: () => api.getFrozenImportPlanSummary(latestRun.data!),
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
        <h1>提交确认</h1>

        {latestRun.isLoading && <p>正在加载可提交的导入任务…</p>}
        {!latestRun.data && !latestRun.isLoading && (
          <div className="commit-empty">
            <p>当前没有可提交的导入任务。</p>
            <button className="btn-primary" onClick={() => onNavigate('scan')}>
              前往扫描
            </button>
          </div>
        )}

        {planQuery.isLoading && <p>正在准备导入计划…</p>}
        {planQuery.isError && (
          <div className="commit-error">
            <p>{String(planQuery.error)}</p>
            <button className="btn-primary" onClick={() => onNavigate('review')}>
              前往审核
            </button>
          </div>
        )}
        {!planQuery.isLoading && !planQuery.isError && latestRun.data && !planQuery.data && (
          <div className="commit-error">
            <p>该导入任务尚未冻结计划。请先在审核页冻结计划后再提交。</p>
            <button className="btn-primary" onClick={() => onNavigate('review')}>
              前往审核
            </button>
            <button className="btn-secondary" onClick={() => onNavigate('scan')}>
              前往扫描
            </button>
          </div>
        )}

        {plan && (
          <div className="commit-confirm">
            <div className="commit-stats">
              <div className="stat-card">
                <div className="stat-value">{plan.total_albums}</div>
                <div className="stat-label">图集数</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{plan.kept_images.length}</div>
                <div className="stat-label">保留图片</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{plan.excluded_count}</div>
                <div className="stat-label">排除</div>
              </div>
              <div className="stat-card">
                <div className="stat-value">{formatFileSize(totalFileSize)}</div>
                <div className="stat-label">预计大小</div>
              </div>
            </div>

            {plan.skipped_albums.length > 0 && (
              <div className="commit-section">
                <h3>跳过的图集</h3>
                <ul>
                  {plan.skipped_albums.map((album) => (
                    <li key={album}>{album}</li>
                  ))}
                </ul>
              </div>
            )}

            <div className="commit-section">
              <h3>待提交文件</h3>
              <table className="commit-table">
                <thead>
                  <tr>
                    <th>图集</th>
                    <th>文件</th>
                    <th>大小</th>
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
                {commitMutation.isPending ? '提交中…' : '提交导入'}
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
        <h1>提交进行中</h1>
        <div className="commit-progress">
          <div className="progress-bar-container">
            <div className="progress-bar" style={{ width: `${pct}%` }} />
          </div>
          <div className="progress-text">{pct}%</div>

          <div className="progress-details">
            <div>阶段: {stageLabel(progress?.current_stage)}</div>
            {progress?.current_album && <div>当前图集: {progress.current_album}</div>}
            <div>
              图集: {progress?.albums_completed ?? 0} / {progress?.albums_total ?? 0}
            </div>
            <div>已提交图片: {progress?.images_committed ?? 0}</div>
            <div>跳过: {progress?.albums_skipped ?? 0}</div>
            <div>失败: {progress?.albums_failed ?? 0}</div>
          </div>

          {/* Detailed staging pipeline progress */}
          <div className="commit-pipeline">
            <h3>流程</h3>
            <ul className="pipeline-steps">
              <li
                className={
                  progress?.current_stage === 'preparing' ? 'active' : progress ? 'done' : ''
                }
              >
                准备事务
              </li>
              <li
                className={
                  progress?.current_stage === 'committing' ||
                  progress?.current_stage === 'processing_album'
                    ? 'active'
                    : progress?.current_stage === 'done' ||
                        progress?.current_stage === 'verifying' ||
                        progress?.current_stage === 'publishing' ||
                        progress?.current_stage === 'db_committing' ||
                        progress?.current_stage === 'library_committed' ||
                        progress?.current_stage === 'source_archiving'
                      ? 'done'
                      : ''
                }
              >
                复制到暂存区
              </li>
              <li
                className={
                  progress?.current_stage === 'verifying'
                    ? 'active'
                    : progress &&
                        [
                          'verified',
                          'publishing',
                          'published',
                          'db_committing',
                          'library_committed',
                          'source_archiving',
                          'source_archived',
                          'done',
                        ].includes(progress.current_stage)
                      ? 'done'
                      : ''
                }
              >
                验证暂存区
              </li>
              <li
                className={
                  progress?.current_stage === 'publishing'
                    ? 'active'
                    : progress &&
                        [
                          'published',
                          'db_committing',
                          'library_committed',
                          'source_archiving',
                          'source_archived',
                          'done',
                        ].includes(progress.current_stage)
                      ? 'done'
                      : ''
                }
              >
                发布目录
              </li>
              <li
                className={
                  progress?.current_stage === 'db_committing'
                    ? 'active'
                    : progress &&
                        [
                          'library_committed',
                          'source_archiving',
                          'source_archived',
                          'done',
                        ].includes(progress.current_stage)
                      ? 'done'
                      : ''
                }
              >
                数据库确认
              </li>
              <li
                className={
                  progress?.current_stage === 'source_archiving'
                    ? 'active'
                    : progress?.current_stage === 'source_archived' ||
                        progress?.current_stage === 'done'
                      ? 'done'
                      : ''
                }
              >
                源图集归档
              </li>
            </ul>
            <div className="progress-details">
              <div>当前阶段: {stageLabel(progress?.current_stage)}</div>
            </div>
          </div>

          {progress && progress.errors.length > 0 && (
            <div className="commit-errors">
              <h3>错误</h3>
              <ul>
                {progress.errors.map((item, index) => (
                  <li key={`${index}-${item}`}>{item}</li>
                ))}
              </ul>
            </div>
          )}

          <div className="commit-actions">
            <button className="btn-danger" onClick={handleCancel}>
              取消
            </button>
          </div>
        </div>
      </div>
    );
  }

  const isSuccess = progress?.state === 'completed';
  const isCancelled = progress?.state === 'cancelled';
  const needsRecovery = progress?.state === 'recovery_required';
  const detectedIncomplete = progress?.state === 'recovery_required';

  return (
    <div className="commit-page">
      <h1>提交结果</h1>

      <div className={`commit-result ${isSuccess ? 'success' : 'partial'}`}>
        <div className="result-status">
          {isSuccess
            ? '导入已完成'
            : isCancelled
              ? '导入已取消'
              : detectedIncomplete
                ? '检测到未完成事务'
                : needsRecovery
                  ? '部分完成，等待恢复'
                  : '导入出现问题'}
        </div>
        <div className="commit-stats">
          <div className="stat-card">
            <div className="stat-value">{progress?.albums_total ?? 0}</div>
            <div className="stat-label">图集数</div>
          </div>
          <div className="stat-card success">
            <div className="stat-value">
              {(progress?.albums_completed ?? 0) -
                (progress?.albums_skipped ?? 0) -
                (progress?.albums_failed ?? 0)}
            </div>
            <div className="stat-label">已提交</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">{progress?.albums_skipped ?? 0}</div>
            <div className="stat-label">跳过</div>
          </div>
          <div className="stat-card danger">
            <div className="stat-value">{progress?.albums_failed ?? 0}</div>
            <div className="stat-label">失败</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">{progress?.images_committed ?? 0}</div>
            <div className="stat-label">图片</div>
          </div>
        </div>
      </div>

      {progress && progress.errors.length > 0 && (
        <div className="commit-errors">
          <h3>错误</h3>
          <ul>
            {progress.errors.map((item, index) => (
              <li key={`${index}-${item}`}>{item}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="commit-actions">
        <button className="btn-primary" onClick={() => onNavigate('dashboard')}>
          返回工作台
        </button>
        {needsRecovery && (
          <button className="btn-secondary" onClick={() => onNavigate('recovery')}>
            前往恢复
          </button>
        )}
      </div>
    </div>
  );
}
