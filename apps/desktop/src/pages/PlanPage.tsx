import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { Route } from '../hooks/use-router';
import { PLAN_ALBUM_BATCH_SIZE, PLAN_IMAGE_BATCH_SIZE } from '../lib/import-plan-ui';
import { api } from '../lib/ipc/api';
import type { ImportPlan, ImportPlanAlbum, ImportPlanImage } from '../lib/ipc/types';
import {
  AppIcon,
  Button,
  EmptyState,
  ImagePreviewDialog,
  PageHeader,
  Skeleton,
  StatusBadge,
  StatusBanner,
} from '../components/ui';

interface PlanPageProps {
  onNavigate: (route: Route) => void;
  onGoCommit?: (importRunId: string) => void;
  onWorkflowAbandoned?: () => void;
  onPlanEditPendingChange?: (pending: boolean) => void;
  initialImportRunId?: string | null;
  initialPlan?: ImportPlan | null;
  enablePolling?: boolean;
}

export interface ImportPlanAlbumGroup {
  albumId: string;
  albumName: string;
  included: boolean;
  imageCount: number;
  skippedImageCount: number;
  totalSize: number;
  images: ImportPlanImage[];
}

export function groupImportPlanImagesByAlbum(images: ImportPlanImage[]): ImportPlanAlbumGroup[] {
  const groups = new Map<string, ImportPlanAlbumGroup>();
  for (const image of images) {
    const current = groups.get(image.album_id) ?? {
      albumId: image.album_id,
      albumName: image.album_name,
      included: true,
      imageCount: 0,
      skippedImageCount: 0,
      totalSize: 0,
      images: [],
    };
    current.images.push(image);
    current.imageCount += image.included ? 1 : 0;
    current.skippedImageCount += image.included ? 0 : 1;
    current.totalSize += image.included ? image.file_size : 0;
    current.included = current.images.some((item) => item.included);
    groups.set(image.album_id, current);
  }
  return [...groups.values()];
}

function planAlbumsForDisplay(plan: ImportPlan): ImportPlanAlbumGroup[] {
  if (plan.albums.length > 0) {
    return plan.albums.map((album: ImportPlanAlbum) => ({
      albumId: album.album_id,
      albumName: album.album_name,
      included: album.included,
      imageCount: album.images.filter((image) => image.included).length,
      skippedImageCount: album.images.filter((image) => !image.included).length,
      totalSize: album.images
        .filter((image) => image.included)
        .reduce((sum, image) => sum + image.file_size, 0),
      images: album.images,
    }));
  }
  return groupImportPlanImagesByAlbum(plan.kept_images);
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

function PlanImageThumbnail({
  importRunId,
  image,
  onOpen,
}: {
  importRunId: string;
  image: ImportPlanImage;
  onOpen: (image: ImportPlanImage, dataUrl: string | null) => void;
}) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .getImportPlanImagePreview(importRunId, image.image_id)
      .then((result) => {
        if (!cancelled) setDataUrl(result.data_url);
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
      {dataUrl ? <img src={dataUrl} alt="" /> : <span>{failed ? '无法预览' : '加载中'}</span>}
    </button>
  );
}

export function PlanPage({
  onNavigate,
  onGoCommit,
  onWorkflowAbandoned,
  onPlanEditPendingChange,
  initialImportRunId = null,
  initialPlan = null,
  enablePolling = true,
}: PlanPageProps) {
  const queryClient = useQueryClient();
  const [importRunId, setImportRunId] = useState<string | null>(
    initialImportRunId ?? initialPlan?.import_run_id ?? null,
  );
  const [plan, setPlan] = useState<ImportPlan | null>(initialPlan);
  const [openAlbums, setOpenAlbums] = useState<Set<string>>(new Set());
  const [imageLimits, setImageLimits] = useState<Record<string, number>>({});
  const [albumLimit, setAlbumLimit] = useState(PLAN_ALBUM_BATCH_SIZE);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmReopen, setConfirmReopen] = useState(false);
  const [confirmAbandon, setConfirmAbandon] = useState(false);
  const [preview, setPreview] = useState<{ image: ImportPlanImage; dataUrl: string | null } | null>(
    null,
  );
  const editActive = useRef(false);

  const latestRun = useQuery({
    queryKey: ['latestReviewableImportRun'],
    queryFn: api.getLatestReviewableImportRun,
    enabled: !importRunId && !initialPlan,
    refetchInterval: enablePolling ? 2000 : false,
  });
  useEffect(() => {
    if (!importRunId && latestRun.data) setImportRunId(latestRun.data);
  }, [importRunId, latestRun.data]);

  const draftQuery = useQuery({
    queryKey: ['importPlanDraftSummary', importRunId],
    queryFn: () => api.getImportPlanDraftSummary(importRunId!),
    enabled: !!importRunId && !initialPlan,
  });
  const frozenQuery = useQuery({
    queryKey: ['frozenImportPlanSummary', importRunId],
    queryFn: () => api.getFrozenImportPlanSummary(importRunId!),
    enabled: !!importRunId && !initialPlan,
  });
  useEffect(() => {
    if (draftQuery.data) setPlan(draftQuery.data);
    else if (frozenQuery.data) setPlan(frozenQuery.data);
  }, [draftQuery.data, frozenQuery.data]);

  const locked = Boolean(plan?.plan_hash);
  const albums = useMemo(() => (plan ? planAlbumsForDisplay(plan) : []), [plan]);
  const keptAlbums = albums.filter((album) => album.included).length;

  const applyEdit = async (edit: () => Promise<ImportPlan>) => {
    if (editActive.current) return;
    editActive.current = true;
    setBusy(true);
    setMessage(null);
    onPlanEditPendingChange?.(true);
    try {
      const next = await edit();
      setPlan(next);
      queryClient.setQueryData(['importPlanDraftSummary', next.import_run_id], next);
    } catch (error) {
      setMessage(String(error));
    } finally {
      editActive.current = false;
      setBusy(false);
      onPlanEditPendingChange?.(false);
    }
  };

  const generate = useMutation({
    mutationFn: () => api.generateImportPlan(importRunId!),
    onSuccess: (next) => {
      setPlan(next);
      queryClient.setQueryData(['importPlanDraftSummary', next.import_run_id], next);
    },
    onError: (error) => setMessage(String(error)),
  });
  const freeze = useMutation({
    mutationFn: () => api.freezeImportPlan(importRunId!),
    onMutate: () => onPlanEditPendingChange?.(true),
    onSuccess: (next) => {
      setPlan(next);
      queryClient.setQueryData(['importPlanDraftSummary', next.import_run_id], null);
      queryClient.setQueryData(['frozenImportPlanSummary', next.import_run_id], next);
    },
    onError: (error) => setMessage(String(error)),
    onSettled: () => onPlanEditPendingChange?.(false),
  });
  const reopen = useMutation({
    mutationFn: () => api.reopenFrozenImportPlan(importRunId!),
    onMutate: () => onPlanEditPendingChange?.(true),
    onSuccess: (next) => {
      setPlan(next);
      setConfirmReopen(false);
      queryClient.setQueryData(['frozenImportPlanSummary', next.import_run_id], null);
      queryClient.setQueryData(['importPlanDraftSummary', next.import_run_id], next);
    },
    onError: (error) => setMessage(String(error)),
    onSettled: () => onPlanEditPendingChange?.(false),
  });

  if (!importRunId && latestRun.isLoading) {
    return (
      <div className="review-page plan-page--m3">
        <PageHeader title="入库调整" description="正在查找可调整的入库任务。" />
        <Skeleton width="100%" height={280} />
      </div>
    );
  }
  if (!importRunId) {
    return (
      <div className="review-page plan-page--m3">
        <PageHeader title="入库调整" />
        <EmptyState
          icon={<AppIcon name="commit" size={30} />}
          title="暂无入库计划"
          description="完成图片审核后，先在这里检查和调整清单，再单独锁定计划。"
          action={<Button onClick={() => onNavigate('review')}>前往审核</Button>}
        />
      </div>
    );
  }
  if (!plan && (draftQuery.isLoading || frozenQuery.isLoading)) {
    return (
      <div className="review-page plan-page--m3">
        <PageHeader title="入库调整" description="正在读取计划。" />
        <Skeleton width="100%" height={280} />
      </div>
    );
  }
  if (!plan) {
    return (
      <div className="review-page plan-page--m3">
        <PageHeader title="入库调整" description="审核完成后，在此生成可编辑草稿。" />
        {message && (
          <StatusBanner tone="danger" title="无法生成计划">
            {message}
          </StatusBanner>
        )}
        <EmptyState
          title="尚未生成入库计划"
          description="生成只会创建草稿，不会锁定计划，也不会开始入库。"
          action={
            <Button
              variant="primary"
              loading={generate.isPending}
              loadingLabel="正在生成…"
              onClick={() => generate.mutate()}
            >
              生成可调整计划
            </Button>
          }
        />
      </div>
    );
  }

  const moveMode = plan.source_file_mode === 'move_selected_without_backup';
  return (
    <div className="review-page plan-page--m3">
      <PageHeader
        title={locked ? '入库计划已锁定' : '入库调整'}
        description={
          locked
            ? '这份计划只读；确认入库阶段只会读取它。尚未创建文件事务时可以恢复为新草稿。'
            : '逐图检查并调整导入选择。只有点击“锁定入库计划”后，才会进入确认入库阶段。'
        }
        meta={
          <StatusBadge tone={locked ? 'success' : 'warning'}>
            {locked ? '计划已锁定' : '可调整草稿'}
          </StatusBadge>
        }
        actions={
          locked ? (
            <Button
              variant="primary"
              onClick={() => (onGoCommit ? onGoCommit(plan.import_run_id) : onNavigate('commit'))}
            >
              前往确认入库
            </Button>
          ) : (
            <Button
              variant="primary"
              disabled={busy || plan.kept_images.length === 0}
              loading={freeze.isPending}
              loadingLabel="正在锁定…"
              onClick={() => freeze.mutate()}
            >
              锁定入库计划
            </Button>
          )
        }
      />

      {message && (
        <StatusBanner tone="danger" title="计划操作失败">
          {message}
        </StatusBanner>
      )}
      {locked && confirmReopen && (
        <StatusBanner
          tone="warning"
          title="恢复为可调整计划？"
          actions={
            <>
              <Button onClick={() => setConfirmReopen(false)}>取消</Button>
              <Button
                variant="primary"
                loading={reopen.isPending}
                loadingLabel="正在恢复…"
                onClick={() => reopen.mutate()}
              >
                确认恢复调整
              </Button>
            </>
          }
        >
          <p>系统会保留当前锁定计划及哈希作为审计证据，并复制一份新的可编辑草稿。</p>
        </StatusBanner>
      )}

      <section className={`plan-source-mode ${moveMode ? 'plan-source-mode--danger' : ''}`}>
        <div>
          <h2>源文件处理</h2>
          <p>
            {moveMode
              ? '移动入库：发布并写入数据库成功后，仅删除计划中已入库的源图片，不创建备份。'
              : '复制并归档：保留默认安全行为，整图集完成后归档源目录。'}
          </p>
        </div>
        <label className="plan-source-mode__toggle">
          <input
            type="checkbox"
            checked={moveMode}
            disabled={busy || locked}
            onChange={(event) =>
              void applyEdit(() =>
                api.setImportPlanSourceFileMode(
                  plan.import_run_id,
                  event.target.checked ? 'move_selected_without_backup' : 'copy_and_archive',
                ),
              )
            }
          />
          <span>移动已选源图片（无备份）</span>
        </label>
        {moveMode && (
          <StatusBanner tone="warning" title="不可撤销的源文件操作">
            仅在发布文件、manifest 和数据库记录全部校验通过后执行。排除图片、sidecar
            和目录不会删除。
          </StatusBanner>
        )}
      </section>

      <section className="import-plan-summary">
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
        </div>
        <div className="plan-list-heading">
          <div>
            <h2>图集清单</h2>
            <p>展开图集后按批加载缩略图和逐图保留/排除操作。</p>
          </div>
          <StatusBadge>
            {keptAlbums} 个图集 · {plan.kept_images.length} 张图片
          </StatusBadge>
        </div>
        <div className="import-plan-albums">
          {albums.slice(0, albumLimit).map((album) => {
            const isOpen = openAlbums.has(album.albumId);
            const imageLimit = imageLimits[album.albumId] ?? PLAN_IMAGE_BATCH_SIZE;
            return (
              <details
                className={`import-plan-album ${album.included ? 'included' : 'skipped'}`}
                key={album.albumId}
                open={isOpen}
                onToggle={(event) => {
                  const nextOpen = event.currentTarget.open;
                  setOpenAlbums((current) => {
                    const next = new Set(current);
                    if (nextOpen) next.add(album.albumId);
                    else next.delete(album.albumId);
                    return next;
                  });
                }}
              >
                <summary>
                  <span className={`import-plan-album-title ${album.included ? '' : 'is-skipped'}`}>
                    {album.albumName}
                  </span>
                  <span className="import-plan-album-meta">
                    导入 {album.imageCount} 张 / 跳过 {album.skippedImageCount} 张 ·{' '}
                    {formatBytes(album.totalSize)}
                  </span>
                  <Button
                    className="plan-album-toggle"
                    variant={album.included ? 'secondary' : 'primary'}
                    disabled={busy || locked}
                    onClick={(event) => {
                      event.preventDefault();
                      void applyEdit(() =>
                        api.setImportPlanAlbumIncluded(
                          plan.import_run_id,
                          album.albumId,
                          !album.included,
                        ),
                      );
                    }}
                  >
                    {album.included ? '排除整组' : '恢复整组'}
                  </Button>
                </summary>
                <div className="import-plan-image-list">
                  {album.images.slice(0, imageLimit).map((image) => (
                    <div
                      className={`import-plan-image-row ${image.included ? '' : 'skipped'}`}
                      key={image.image_id}
                    >
                      <PlanImageThumbnail
                        importRunId={plan.import_run_id}
                        image={image}
                        onOpen={(nextImage, dataUrl) => setPreview({ image: nextImage, dataUrl })}
                      />
                      <span className="import-plan-image-info">
                        <strong>{image.relative_path}</strong>
                        <small>{formatBytes(image.file_size)}</small>
                      </span>
                      <Button
                        className={`plan-toggle ${image.included ? 'is-on' : 'is-off'}`}
                        variant={image.included ? 'secondary' : 'primary'}
                        disabled={busy || locked}
                        onClick={() =>
                          void applyEdit(() =>
                            api.setImportPlanImageIncluded(
                              plan.import_run_id,
                              image.image_id,
                              album.albumId,
                              !image.included,
                            ),
                          )
                        }
                      >
                        {image.included ? '排除' : '保留'}
                      </Button>
                    </div>
                  ))}
                  {imageLimit < album.images.length && (
                    <Button
                      onClick={() =>
                        setImageLimits((current) => ({
                          ...current,
                          [album.albumId]: imageLimit + PLAN_IMAGE_BATCH_SIZE,
                        }))
                      }
                    >
                      加载更多图片
                    </Button>
                  )}
                </div>
              </details>
            );
          })}
        </div>
        {albumLimit < albums.length && (
          <Button onClick={() => setAlbumLimit((value) => value + PLAN_ALBUM_BATCH_SIZE)}>
            加载更多图集
          </Button>
        )}
      </section>

      {confirmAbandon && (
        <StatusBanner
          tone="danger"
          title="确认放弃这次导入？"
          actions={
            <>
              <Button onClick={() => setConfirmAbandon(false)}>取消</Button>
              <Button
                variant="danger"
                onClick={() =>
                  void api
                    .abandonFrozenImportWorkflow(plan.import_run_id)
                    .then(() => {
                      onWorkflowAbandoned?.();
                      onNavigate('dashboard');
                    })
                    .catch((error) => setMessage(String(error)))
                }
              >
                确认放弃
              </Button>
            </>
          }
        >
          <p>这会终止当前工作流，但不会删除源文件。</p>
        </StatusBanner>
      )}
      <div className="plan-footer-actions">
        <Button variant="danger" disabled={busy} onClick={() => setConfirmAbandon(true)}>
          放弃这次导入
        </Button>
        {locked ? (
          <>
            <Button disabled={busy} onClick={() => setConfirmReopen(true)}>
              恢复入库调整
            </Button>
            <Button
              variant="primary"
              onClick={() => (onGoCommit ? onGoCommit(plan.import_run_id) : onNavigate('commit'))}
            >
              前往确认入库
            </Button>
          </>
        ) : (
          <Button
            variant="primary"
            disabled={busy || plan.kept_images.length === 0}
            loading={freeze.isPending}
            loadingLabel="正在锁定…"
            onClick={() => freeze.mutate()}
          >
            锁定入库计划
          </Button>
        )}
      </div>

      {preview && (
        <ImagePreviewDialog
          dataUrl={preview.dataUrl}
          path={preview.image.relative_path}
          onClose={() => setPreview(null)}
        />
      )}
    </div>
  );
}
