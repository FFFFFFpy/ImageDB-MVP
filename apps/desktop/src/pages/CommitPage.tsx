import { useCallback, useEffect, useRef, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type {
  CommitProgress,
  ImportPlan,
  ImportPlanAlbum,
  ImportPlanImage,
} from '../lib/ipc/types';

interface CommitPageProps {
  onNavigate: (route: Route) => void;
}

type Phase = 'confirm' | 'committing' | 'result';

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

interface CommitPlanAlbumGroup {
  albumId: string;
  albumName: string;
  included: boolean;
  imageCount: number;
  skippedImageCount: number;
  totalSize: number;
  images: ImportPlanImage[];
}

function planAlbumsForDisplay(plan: ImportPlan): CommitPlanAlbumGroup[] {
  if (plan.albums?.length) {
    return plan.albums.map((album: ImportPlanAlbum) => ({
      albumId: album.album_id,
      albumName: album.album_name,
      included: album.included,
      imageCount: album.image_count,
      skippedImageCount: album.images.filter((image) => !image.included).length,
      totalSize: album.total_size,
      images: album.images,
    }));
  }

  const groups = new Map<string, CommitPlanAlbumGroup>();
  plan.kept_images.forEach((image) => {
    const albumId = image.album_id || image.album_name || 'unknown';
    const existing = groups.get(albumId);
    if (existing) {
      existing.imageCount += 1;
      existing.totalSize += image.file_size;
      existing.images.push(image);
      return;
    }
    groups.set(albumId, {
      albumId,
      albumName: image.album_name || '未命名图集',
      included: true,
      imageCount: 1,
      skippedImageCount: 0,
      totalSize: image.file_size,
      images: [image],
    });
  });
  return Array.from(groups.values());
}

interface CommitImageThumbnailProps {
  importRunId: string;
  image: ImportPlanImage;
  onOpen: (image: ImportPlanImage, dataUrl: string | null) => void;
}

function CommitImageThumbnail({ importRunId, image, onOpen }: CommitImageThumbnailProps) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .getImportPlanImagePreview(importRunId, image.image_id)
      .then((preview) => {
        if (!cancelled) setDataUrl(preview.data_url);
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [image.image_id, importRunId]);

  return (
    <button
      type="button"
      className="import-plan-thumb"
      onClick={() => onOpen(image, dataUrl)}
      aria-label={`预览 ${image.relative_path}`}
    >
      {dataUrl ? (
        <img src={dataUrl} alt="" loading="lazy" />
      ) : (
        <span>{failed ? '无预览' : '加载中'}</span>
      )}
    </button>
  );
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
  const [openPlanAlbums, setOpenPlanAlbums] = useState<Set<string>>(new Set());
  const [previewModal, setPreviewModal] = useState<{
    image: ImportPlanImage;
    dataUrl: string | null;
  } | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Keep this aligned with ImportRepository::get_latest_committable_run:
  // only `ready_to_commit` and resubmittable `cancelled` runs with a frozen
  // or consumed plan and non-empty plan_hash enter the default Commit page.
  // `completed`, `failed`, and `recovery_required` do not.
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

  const openPlanImagePreview = useCallback(
    (image: ImportPlanImage, dataUrl: string | null) => {
      setPreviewModal({ image, dataUrl });
      if (dataUrl || !importRunId) return;
      api
        .getImportPlanImagePreview(importRunId, image.image_id)
        .then((preview) => {
          setPreviewModal((current) =>
            current?.image.image_id === image.image_id
              ? { image, dataUrl: preview.data_url }
              : current,
          );
        })
        .catch(() => {
          setPreviewModal((current) =>
            current?.image.image_id === image.image_id ? { image, dataUrl: null } : current,
          );
        });
    },
    [importRunId],
  );

  const totalFileSize = plan
    ? plan.kept_images.reduce((sum, image) => sum + image.file_size, 0)
    : 0;

  if (phase === 'confirm') {
    const albumGroups = plan ? planAlbumsForDisplay(plan) : [];
    const keptAlbums = albumGroups.filter((album) => album.included).length;

    return (
      <div className="commit-page">
        {plan ? (
          <div className="import-plan-header">
            <h1>提交确认</h1>
            <div className="toolbar import-plan-actions">
              <button
                className="btn-primary"
                onClick={handleStartCommit}
                disabled={commitMutation.isPending || plan.kept_images.length === 0}
              >
                {commitMutation.isPending ? '提交中…' : '提交导入'}
              </button>
              <button className="btn-secondary" onClick={() => onNavigate('review')}>
                退回至导入计划
              </button>
            </div>
          </div>
        ) : (
          <h1>提交确认</h1>
        )}

        {latestRun.isLoading && <p>正在加载可提交的导入任务…</p>}
        {!latestRun.data && !latestRun.isLoading && (
          <div className="commit-empty">
            <p>当前没有已冻结计划的可提交任务。</p>
            <button className="btn-primary" onClick={() => onNavigate('review')}>
              前往审核 / 生成导入计划
            </button>
            <button className="btn-secondary" onClick={() => onNavigate('scan')}>
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
            <div className="import-plan-summary">
              <div className="import-plan-stats">
                <div className="scan-progress-card">
                  <h3>图集数</h3>
                  <p>{plan.total_albums}</p>
                </div>
                <div className="scan-progress-card">
                  <h3>图片总数</h3>
                  <p>{plan.total_images}</p>
                </div>
                <div className="scan-progress-card ok">
                  <h3>导入</h3>
                  <p>{plan.kept_images.length}</p>
                </div>
                <div className="scan-progress-card warn">
                  <h3>排除</h3>
                  <p>{plan.excluded_count}</p>
                </div>
                <div className="scan-progress-card">
                  <h3>预计大小</h3>
                  <p>{formatFileSize(totalFileSize)}</p>
                </div>
              </div>

              <div className="import-plan-kept">
                <h3>
                  导入图集 ({keptAlbums}) / 导入图片 ({plan.kept_images.length})
                </h3>
                <div className="import-plan-albums">
                  {albumGroups.map((album) => {
                    const isOpen = openPlanAlbums.has(album.albumId);
                    return (
                      <details
                        className={`import-plan-album ${album.included ? 'included' : 'skipped'}`}
                        key={album.albumId}
                        open={isOpen}
                        onToggle={(event) => {
                          const nextOpen = event.currentTarget.open;
                          setOpenPlanAlbums((current) => {
                            const next = new Set(current);
                            if (nextOpen) next.add(album.albumId);
                            else next.delete(album.albumId);
                            return next;
                          });
                        }}
                      >
                        <summary>
                          <span
                            className={`import-plan-album-title ${
                              album.included ? '' : 'is-skipped'
                            }`}
                          >
                            {album.albumName}
                          </span>
                          <span className="import-plan-album-meta">
                            导入 {album.imageCount} 张 / 跳过 {album.skippedImageCount} 张 ·{' '}
                            {formatFileSize(album.totalSize)}
                          </span>
                          <span className={`plan-toggle ${album.included ? 'is-on' : 'is-off'}`}>
                            {album.included ? '导入' : '跳过'}
                          </span>
                        </summary>
                        {isOpen && (
                          <div className="import-plan-image-list">
                            {album.images.map((image) => (
                              <div
                                className={`import-plan-image-row ${
                                  image.included ? 'included' : 'skipped'
                                }`}
                                key={image.image_id}
                              >
                                <CommitImageThumbnail
                                  importRunId={plan.import_run_id}
                                  image={image}
                                  onOpen={openPlanImagePreview}
                                />
                                <button
                                  type="button"
                                  className="import-plan-image-info"
                                  onClick={() => openPlanImagePreview(image, null)}
                                >
                                  <span className="mono">{image.relative_path}</span>
                                  <span>{formatFileSize(image.file_size)}</span>
                                </button>
                                <span
                                  className={`plan-toggle ${image.included ? 'is-on' : 'is-off'}`}
                                >
                                  {image.included ? '导入' : '跳过'}
                                </span>
                              </div>
                            ))}
                          </div>
                        )}
                      </details>
                    );
                  })}
                </div>
              </div>
            </div>

            {previewModal && (
              <div className="image-preview-modal" onClick={() => setPreviewModal(null)}>
                <div className="image-preview-dialog" onClick={(event) => event.stopPropagation()}>
                  {previewModal.dataUrl ? (
                    <img src={previewModal.dataUrl} alt={previewModal.image.relative_path} />
                  ) : (
                    <div className="image-preview-loading">正在加载预览...</div>
                  )}
                  <div className="mono">{previewModal.image.relative_path}</div>
                </div>
              </div>
            )}

            {error && <div className="commit-error-msg">{error}</div>}
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
            <div className="progress-bar" style={{ transform: `scaleX(${pct / 100})` }} />
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
