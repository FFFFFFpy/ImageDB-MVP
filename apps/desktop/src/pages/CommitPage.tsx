import { useCallback, useEffect, useRef, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { PLAN_ALBUM_BATCH_SIZE, PLAN_IMAGE_BATCH_SIZE } from '../lib/import-plan-ui';
import type { Route } from '../hooks/use-router';
import type {
  CommitProgress,
  ImportPlan,
  ImportPlanAlbum,
  ImportPlanImage,
} from '../lib/ipc/types';
import {
  Button,
  EmptyState,
  ImagePreviewDialog,
  PageHeader,
  Progress,
  Skeleton,
  StatusBadge,
  StatusBanner,
  StatusIcon,
} from '../components/ui';

interface CommitPageProps {
  onNavigate: (route: Route) => void;
  onGoReview?: (importRunId: string) => void;
  initialPhase?: Phase;
  initialPlan?: ImportPlan | null;
  initialProgress?: CommitProgress | null;
  initialImportRunId?: string | null;
  enablePolling?: boolean;
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

export function isTerminalProgress(progress: CommitProgress): boolean {
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

export const COMMIT_PIPELINE = [
  { key: 'preparing', label: '准备事务' },
  { key: 'staging', label: '复制到暂存区' },
  { key: 'verifying', label: '验证暂存区' },
  { key: 'publishing', label: '发布目录' },
  { key: 'db', label: '数据库确认' },
  { key: 'archiving', label: '源图集归档' },
] as const;

const stageOrder: Record<string, number> = {
  preparing: 0,
  committing: 1,
  processing_album: 1,
  verifying: 2,
  verified: 3,
  publishing: 3,
  published: 4,
  db_committing: 4,
  library_committed: 5,
  source_archiving: 5,
  source_archived: 6,
  done: 6,
};

export function commitPipelineStepState(
  currentStage: string | undefined,
  stepIndex: number,
): 'pending' | 'active' | 'done' {
  if (!currentStage || !(currentStage in stageOrder)) return 'pending';
  const currentIndex = stageOrder[currentStage];
  if (currentIndex > stepIndex) return 'done';
  if (currentIndex === stepIndex) return 'active';
  return 'pending';
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

export function CommitPage({
  onNavigate,
  onGoReview,
  initialPhase = 'confirm',
  initialPlan = null,
  initialProgress = null,
  initialImportRunId = null,
  enablePolling = true,
}: CommitPageProps) {
  const [phase, setPhase] = useState<Phase>(initialPhase);
  const [plan, setPlan] = useState<ImportPlan | null>(initialPlan);
  const [progress, setProgress] = useState<CommitProgress | null>(initialProgress);
  const [error, setError] = useState<string | null>(null);
  const [importRunId, setImportRunId] = useState<string | null>(initialImportRunId);
  const [openPlanAlbums, setOpenPlanAlbums] = useState<Set<string>>(new Set());
  const [planImageLimits, setPlanImageLimits] = useState<Record<string, number>>({});
  const [planAlbumLimit, setPlanAlbumLimit] = useState(PLAN_ALBUM_BATCH_SIZE);
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
    enabled: !initialImportRunId,
  });

  const committableRunId = latestRun.data ?? importRunId;

  const planQuery = useQuery({
    queryKey: ['frozenImportPlanSummary', committableRunId],
    queryFn: () => api.getFrozenImportPlanSummary(committableRunId!),
    enabled: !!committableRunId && phase === 'confirm' && !initialPlan,
  });

  useEffect(() => {
    if (planQuery.data) {
      setPlan(planQuery.data);
      setImportRunId(planQuery.data.import_run_id);
    }
  }, [planQuery.data]);

  useEffect(() => {
    if (phase !== 'committing' || !enablePolling) return;

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
  }, [phase, enablePolling]);

  const commitMutation = useMutation({
    mutationFn: ({ runId, planHash }: { runId: string; planHash: string }) =>
      api.startImportCommit(runId, planHash),
    onSuccess: () => {
      setError(null);
      setPhase('committing');
    },
    onError: (err) => {
      setError(String(err));
    },
  });

  const handleStartCommit = useCallback(() => {
    if (!importRunId || !plan?.plan_hash) return;
    commitMutation.mutate({ runId: importRunId, planHash: plan.plan_hash });
  }, [commitMutation, importRunId, plan?.plan_hash]);

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
      <div className="commit-page commit-page--m3">
        <PageHeader
          title="提交入库"
          description="确认 frozen plan 摘要后，才会开始 staging、校验、发布与数据库提交。"
          meta={plan ? <StatusBadge tone="success">读取 frozen plan</StatusBadge> : undefined}
          actions={
            plan ? (
              <>
                <Button
                  variant="quiet"
                  onClick={() =>
                    committableRunId && onGoReview
                      ? onGoReview(committableRunId)
                      : onNavigate('review')
                  }
                >
                  退回导入计划
                </Button>
                <Button
                  variant="primary"
                  onClick={handleStartCommit}
                  disabled={plan.kept_images.length === 0 || !plan.plan_hash}
                  loading={commitMutation.isPending}
                  loadingLabel="正在启动…"
                >
                  确认并开始入库
                </Button>
              </>
            ) : undefined
          }
        />

        {plan && (
          <StatusBanner
            tone={plan.plan_hash ? 'info' : 'danger'}
            title={plan.plan_hash ? '已锁定本次提交计划' : '计划缺少完整性哈希'}
          >
            {plan.plan_hash
              ? `计划哈希：${plan.plan_hash}`
              : '无法确认当前页面展示的计划与后端即将提交的计划一致，提交已阻止。请返回审核页重新生成 frozen plan。'}
          </StatusBanner>
        )}

        {latestRun.isLoading && !plan && (
          <div className="commit-loading" role="status" aria-label="正在加载可提交的导入任务">
            <Skeleton height={96} radius="var(--radius-panel)" />
          </div>
        )}
        {!committableRunId && !latestRun.isLoading && !plan && (
          <EmptyState
            title="没有可提交的计划"
            description="当前没有已冻结计划的可提交任务。请先完成审核并生成导入计划。"
            action={
              <div className="commit-empty-actions">
                <Button
                  variant="primary"
                  onClick={() =>
                    committableRunId && onGoReview
                      ? onGoReview(committableRunId)
                      : onNavigate('review')
                  }
                >
                  前往审核 / 生成计划
                </Button>
                <Button variant="quiet" onClick={() => onNavigate('scan')}>
                  前往扫描
                </Button>
              </div>
            }
          />
        )}

        {planQuery.isLoading && !plan && (
          <div className="commit-loading" role="status" aria-label="正在读取 frozen plan">
            <Skeleton height={160} radius="var(--radius-panel)" />
          </div>
        )}
        {planQuery.isError && (
          <StatusBanner
            tone="danger"
            title="无法读取 frozen plan"
            actions={<Button onClick={() => onNavigate('review')}>前往审核</Button>}
          >
            {String(planQuery.error)}
          </StatusBanner>
        )}
        {!planQuery.isLoading &&
          !planQuery.isError &&
          committableRunId &&
          !planQuery.data &&
          !plan && (
            <StatusBanner
              tone="warning"
              title="计划尚未冻结"
              actions={<Button onClick={() => onNavigate('review')}>返回审核</Button>}
            >
              该导入任务没有可读取的 frozen plan；为保证提交输入稳定，入库操作已阻止。
            </StatusBanner>
          )}

        {plan && (
          <div className="commit-confirm">
            <StatusBanner tone="warning" title="开始后将写入文件系统与数据库">
              发布成功并完成完整性校验前不会归档源图集；取消后可能需要通过恢复页继续处理。
            </StatusBanner>
            <div className="import-plan-summary">
              <div className="import-plan-stats">
                <div className="plan-stat">
                  <span>图集数</span>
                  <strong>{plan.total_albums}</strong>
                </div>
                <div className="plan-stat">
                  <span>图片总数</span>
                  <strong>{plan.total_images}</strong>
                </div>
                <div className="plan-stat plan-stat--success">
                  <span>计划导入</span>
                  <strong>{plan.kept_images.length}</strong>
                </div>
                <div className="plan-stat plan-stat--warning">
                  <span>计划排除</span>
                  <strong>{plan.excluded_count}</strong>
                </div>
                <div className="plan-stat">
                  <span>预计大小</span>
                  <strong>{formatFileSize(totalFileSize)}</strong>
                </div>
              </div>

              <div className="import-plan-kept">
                <div className="plan-list-heading">
                  <div>
                    <h2>只读提交清单</h2>
                    <p>内容来自 frozen plan，不在此页重新计算。</p>
                  </div>
                  <StatusBadge>
                    {keptAlbums} 个图集 · {plan.kept_images.length} 张图片
                  </StatusBadge>
                </div>
                <div className="import-plan-albums">
                  {albumGroups.slice(0, planAlbumLimit).map((album) => {
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
                            {album.images
                              .slice(0, planImageLimits[album.albumId] ?? PLAN_IMAGE_BATCH_SIZE)
                              .map((image) => (
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
                            {album.images.length > (planImageLimits[album.albumId] ?? 24) && (
                              <Button
                                variant="quiet"
                                className="plan-load-more"
                                onClick={() =>
                                  setPlanImageLimits((current) => ({
                                    ...current,
                                    [album.albumId]:
                                      (current[album.albumId] ?? PLAN_IMAGE_BATCH_SIZE) +
                                      PLAN_IMAGE_BATCH_SIZE,
                                  }))
                                }
                              >
                                再显示 {PLAN_IMAGE_BATCH_SIZE} 张（剩余{' '}
                                {album.images.length -
                                  (planImageLimits[album.albumId] ?? PLAN_IMAGE_BATCH_SIZE)}{' '}
                                张）
                              </Button>
                            )}
                          </div>
                        )}
                      </details>
                    );
                  })}
                  {albumGroups.length > planAlbumLimit && (
                    <Button
                      variant="quiet"
                      className="plan-load-more"
                      onClick={() =>
                        setPlanAlbumLimit((current) => current + PLAN_ALBUM_BATCH_SIZE)
                      }
                    >
                      再显示 {PLAN_ALBUM_BATCH_SIZE} 个图集（剩余{' '}
                      {albumGroups.length - planAlbumLimit} 个）
                    </Button>
                  )}
                </div>
              </div>
            </div>

            {previewModal && (
              <ImagePreviewDialog
                dataUrl={previewModal.dataUrl}
                path={previewModal.image.relative_path}
                onClose={() => setPreviewModal(null)}
              />
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
      <div className="commit-page commit-page--m3">
        <PageHeader
          title="正在入库"
          description="请保持 ImageDB 运行；当前阶段与持久化事务状态同步。"
          meta={<StatusBadge tone="info">{stageLabel(progress?.current_stage)}</StatusBadge>}
          actions={
            <Button variant="danger" onClick={handleCancel}>
              取消入库
            </Button>
          }
        />

        <section className="commit-progress-panel" aria-labelledby="commit-progress-title">
          <div className="commit-progress-heading">
            <div>
              <h2 id="commit-progress-title">{progress?.current_album ?? '正在准备首个图集'}</h2>
              <p>
                已处理 {progress?.albums_completed ?? 0} / {progress?.albums_total ?? 0} 个图集
              </p>
            </div>
            <strong>{pct}%</strong>
          </div>
          <Progress
            value={pct}
            label="整体入库进度"
            detail={`${progress?.images_committed ?? 0} 张图片已提交`}
          />
          <dl className="commit-metrics">
            <div>
              <dt>已提交图片</dt>
              <dd>{progress?.images_committed ?? 0}</dd>
            </div>
            <div>
              <dt>已跳过图集</dt>
              <dd>{progress?.albums_skipped ?? 0}</dd>
            </div>
            <div>
              <dt>失败图集</dt>
              <dd>{progress?.albums_failed ?? 0}</dd>
            </div>
          </dl>
        </section>

        <section className="commit-pipeline" aria-labelledby="commit-pipeline-title">
          <div className="commit-section-heading">
            <div>
              <h2 id="commit-pipeline-title">文件事务流程</h2>
              <p>每一步完成后才会进入下一阶段。</p>
            </div>
          </div>
          <ol className="pipeline-steps">
            {COMMIT_PIPELINE.map((step, index) => {
              const state = commitPipelineStepState(progress?.current_stage, index);
              return (
                <li className={state} key={step.key}>
                  <StatusIcon name={state === 'done' ? 'check' : 'info'} size={17} />
                  <span>{step.label}</span>
                </li>
              );
            })}
          </ol>
        </section>

        {progress && progress.errors.length > 0 && (
          <StatusBanner tone="danger" title="入库过程中出现错误">
            {progress.errors.join('；')}
          </StatusBanner>
        )}
        {error && (
          <StatusBanner tone="danger" title="操作失败">
            {error}
          </StatusBanner>
        )}
      </div>
    );
  }

  const isSuccess = progress?.state === 'completed';
  const isCancelled = progress?.state === 'cancelled';
  const needsRecovery = progress?.state === 'recovery_required';
  const resultTitle = isSuccess
    ? '导入已完成'
    : isCancelled
      ? '导入已取消'
      : needsRecovery
        ? '检测到未完成事务'
        : '导入出现问题';

  return (
    <div className="commit-page commit-page--m3">
      <PageHeader
        title="入库结果"
        description="结果来自持久化事务状态；需要恢复时不会将任务显示为成功。"
        actions={<Button onClick={() => onNavigate('dashboard')}>返回工作台</Button>}
      />

      <section className={`commit-result ${isSuccess ? 'success' : 'partial'}`}>
        <StatusIcon name={isSuccess ? 'check' : needsRecovery ? 'warning' : 'error'} size={32} />
        <div className="commit-result-copy">
          <h2>{resultTitle}</h2>
          <p>
            {isSuccess
              ? '文件发布、数据库提交与源图集归档均已完成。'
              : needsRecovery
                ? '存在需要继续核对的文件事务，请前往恢复页处理。'
                : isCancelled
                  ? '任务已停止；源文件处理状态以当前持久化记录为准。'
                  : '任务没有完成，请查看错误并保留现场。'}
          </p>
        </div>
        <div className="commit-stats">
          <div className="plan-stat">
            <span>图集数</span>
            <strong>{progress?.albums_total ?? 0}</strong>
          </div>
          <div className="plan-stat plan-stat--success">
            <span>已提交</span>
            <strong>
              {(progress?.albums_completed ?? 0) -
                (progress?.albums_skipped ?? 0) -
                (progress?.albums_failed ?? 0)}
            </strong>
          </div>
          <div className="plan-stat">
            <span>跳过</span>
            <strong>{progress?.albums_skipped ?? 0}</strong>
          </div>
          <div className="plan-stat plan-stat--warning">
            <span>失败</span>
            <strong>{progress?.albums_failed ?? 0}</strong>
          </div>
          <div className="plan-stat">
            <span>图片</span>
            <strong>{progress?.images_committed ?? 0}</strong>
          </div>
        </div>
      </section>

      {progress && progress.errors.length > 0 && (
        <StatusBanner tone="danger" title="错误详情">
          {progress.errors.join('；')}
        </StatusBanner>
      )}

      {needsRecovery && (
        <div className="commit-result-actions">
          <Button variant="primary" onClick={() => onNavigate('recovery')}>
            前往恢复
          </Button>
        </div>
      )}
    </div>
  );
}
