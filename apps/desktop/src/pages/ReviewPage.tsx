import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { PLAN_ALBUM_BATCH_SIZE, PLAN_IMAGE_BATCH_SIZE } from '../lib/import-plan-ui';
import type { Route } from '../hooks/use-router';
import type {
  ReviewCandidateDetail,
  ReviewCandidateSummary,
  ReviewDecision,
  ImportPlan,
  ImportPlanAlbum,
  ImportPlanImage,
} from '../lib/ipc/types';
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

interface ReviewPageProps {
  onNavigate: (route: Route) => void;
  initialImportRunId?: string | null;
  initialPreviews?: { left: string; right: string } | null;
  initialPlan?: ImportPlan | null;
  initialShowPlan?: boolean;
  enablePolling?: boolean;
}

interface ViewState {
  scale: number;
  offsetX: number;
  offsetY: number;
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

const DEFAULT_VIEW: ViewState = { scale: 1, offsetX: 0, offsetY: 0 };

export const REVIEW_DECISION_OPTIONS: ReadonlyArray<{
  decision: ReviewDecision;
  shortcut: string;
  label: string;
}> = [
  { decision: 'keep_source', shortcut: '1', label: '保留源图片' },
  { decision: 'keep_candidate', shortcut: '2', label: '保留候选图片' },
  { decision: 'keep_all', shortcut: '3', label: '全部保留' },
];

type ReviewInvalidationClient = Pick<QueryClient, 'invalidateQueries'>;

export function invalidateReviewWorkflowQueries(queryClient: ReviewInvalidationClient) {
  queryClient.invalidateQueries({ queryKey: ['reviewQueue'] });
  queryClient.invalidateQueries({ queryKey: ['reviewProgress'] });
  queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] });
  queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] });
  queryClient.invalidateQueries({ queryKey: ['import-run-albums'] });
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function groupImportPlanImagesByAlbum(images: ImportPlanImage[]): ImportPlanAlbumGroup[] {
  const groups = new Map<string, ImportPlanAlbumGroup>();

  images.forEach((image) => {
    const albumId = image.album_id || image.album_name || 'unknown';
    const albumName = image.album_name || '未命名图集';
    const existing = groups.get(albumId);
    if (existing) {
      existing.imageCount += image.included ? 1 : 0;
      existing.skippedImageCount += image.included ? 0 : 1;
      existing.totalSize += image.included ? image.file_size : 0;
      existing.images.push(image);
      existing.included = existing.included || image.included;
      return;
    }

    groups.set(albumId, {
      albumId,
      albumName,
      included: image.included,
      imageCount: image.included ? 1 : 0,
      skippedImageCount: image.included ? 0 : 1,
      totalSize: image.included ? image.file_size : 0,
      images: [image],
    });
  });

  return Array.from(groups.values());
}

function planAlbumsForDisplay(plan: ImportPlan): ImportPlanAlbumGroup[] {
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
  return groupImportPlanImagesByAlbum(plan.kept_images);
}

interface PlanImageThumbnailProps {
  importRunId: string;
  image: ImportPlanImage;
  onOpen: (image: ImportPlanImage, dataUrl: string | null) => void;
}

function PlanImageThumbnail({ importRunId, image, onOpen }: PlanImageThumbnailProps) {
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

function formatDistance(val: number | null): string {
  if (val === null) return '无';
  return val.toString();
}

function formatMatchType(t: string): string {
  const map: Record<string, string> = {
    file_exact: '文件完全一致',
    pixel_exact: '像素完全一致',
    perceptual_near: '感知近似',
    perceptual_similar: '感知相似',
  };
  return map[t] ?? t;
}

function formatScope(s: string): string {
  const map: Record<string, string> = {
    intra_album: '图集内',
    cross_album: '跨图集',
    library: '历史图库',
  };
  return map[s] ?? s;
}

function formatTransform(t: string | null): string {
  if (!t) return '无';
  const map: Record<string, string> = {
    identity: '不变',
    rot90: '旋转 90 度',
    rot180: '旋转 180 度',
    rot270: '旋转 270 度',
    flip_h: '水平翻转',
    flip_v: '垂直翻转',
    transpose: '主对角线翻转',
    transverse: '副对角线翻转',
  };
  return map[t] ?? t;
}

export function ReviewPage({
  onNavigate,
  initialImportRunId = null,
  initialPreviews = null,
  initialPlan = null,
  initialShowPlan = false,
  enablePolling = true,
}: ReviewPageProps) {
  const queryClient = useQueryClient();

  const [importRunId, setImportRunId] = useState<string | null>(initialImportRunId);
  const [currentIndex, setCurrentIndex] = useState(0);
  const [overlayMode, setOverlayMode] = useState(false);
  const [overlayOpacity, setOverlayOpacity] = useState(0.5);
  const [view, setView] = useState<ViewState>(DEFAULT_VIEW);
  const [isPanning, setIsPanning] = useState(false);
  const [panStart, setPanStart] = useState({ x: 0, y: 0 });
  const [leftPreview, setLeftPreview] = useState<string | null>(null);
  const [rightPreview, setRightPreview] = useState<string | null>(null);
  const [importPlan, setImportPlan] = useState<ImportPlan | null>(initialPlan);
  const [showPlan, setShowPlan] = useState(initialShowPlan);
  const [openPlanAlbums, setOpenPlanAlbums] = useState<Set<string>>(new Set());
  const [planImageLimits, setPlanImageLimits] = useState<Record<string, number>>({});
  const [planAlbumLimit, setPlanAlbumLimit] = useState(PLAN_ALBUM_BATCH_SIZE);
  const [planEditError, setPlanEditError] = useState<string | null>(null);
  const [planEditPending, setPlanEditPending] = useState(false);
  const [previewModal, setPreviewModal] = useState<{
    image: ImportPlanImage;
    dataUrl: string | null;
  } | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const containerRef = useRef<HTMLDivElement>(null);

  const runQuery = useQuery({
    queryKey: ['latestReviewableImportRun'],
    queryFn: () => api.getLatestReviewableImportRun(),
    enabled: !importRunId,
  });

  useEffect(() => {
    if (runQuery.data && !importRunId) {
      setImportRunId(runQuery.data);
    }
  }, [runQuery.data, importRunId]);

  const queueQuery = useQuery({
    queryKey: ['reviewQueue', importRunId],
    queryFn: () => api.getReviewQueue(importRunId!),
    enabled: !!importRunId,
  });

  const progressQuery = useQuery({
    queryKey: ['reviewProgress', importRunId],
    queryFn: () => api.getReviewProgress(importRunId!),
    enabled: !!importRunId,
    refetchInterval: enablePolling ? 5000 : false,
  });

  const frozenPlanQuery = useQuery({
    queryKey: ['reviewFrozenImportPlanSummary', importRunId],
    queryFn: () => api.getFrozenImportPlanSummary(importRunId!),
    enabled: !!importRunId && !showPlan && !importPlan && !initialPlan,
  });

  useEffect(() => {
    if (frozenPlanQuery.data && !showPlan && !importPlan) {
      setImportPlan(frozenPlanQuery.data);
      setShowPlan(true);
      setOpenPlanAlbums(new Set());
      setPlanEditError(null);
    }
  }, [frozenPlanQuery.data, importPlan, showPlan]);

  const queue = queueQuery.data ?? [];
  const undecidedQueue = useMemo(() => queue.filter((c) => !c.has_decision), [queue]);
  const reviewAlbumCount = useMemo(
    () => new Set(undecidedQueue.map((candidate) => candidate.album_name)).size,
    [undecidedQueue],
  );

  const currentCandidate: ReviewCandidateSummary | undefined = undecidedQueue[currentIndex];

  const detailQuery = useQuery({
    queryKey: ['reviewDetail', currentCandidate?.candidate_id],
    queryFn: () => api.getReviewCandidateDetail(currentCandidate!.candidate_id),
    enabled: !!currentCandidate,
  });

  const detail = detailQuery.data ?? null;

  useEffect(() => {
    if (!detail) {
      setLeftPreview(null);
      setRightPreview(null);
      return;
    }
    if (initialPreviews) {
      setLeftPreview(initialPreviews.left);
      setRightPreview(initialPreviews.right);
      return;
    }
    let cancelled = false;

    api
      .getImagePreview(detail.candidate_id, 'source')
      .then((p) => {
        if (!cancelled) setLeftPreview(p.data_url);
      })
      .catch(() => {
        if (!cancelled) setLeftPreview(null);
      });

    api
      .getImagePreview(detail.candidate_id, 'candidate')
      .then((p) => {
        if (!cancelled) setRightPreview(p.data_url);
      })
      .catch(() => {
        if (!cancelled) setRightPreview(null);
      });

    return () => {
      cancelled = true;
    };
  }, [detail?.candidate_id, initialPreviews]);

  useEffect(() => {
    setView(DEFAULT_VIEW);
    setOverlayMode(false);
  }, [currentCandidate?.candidate_id]);

  const submitDecision = useMutation({
    mutationFn: ({ candidateId, decision }: { candidateId: string; decision: ReviewDecision }) =>
      api.submitReviewDecision(candidateId, decision),
    onSuccess: () => {
      invalidateReviewWorkflowQueries(queryClient);
      setSubmitting(false);
    },
    onError: () => {
      setSubmitting(false);
    },
  });

  const handleDecision = useCallback(
    async (decision: ReviewDecision) => {
      if (!currentCandidate || submitting) return;
      setSubmitting(true);
      try {
        await submitDecision.mutateAsync({
          candidateId: currentCandidate.candidate_id,
          decision,
        });
        if (currentIndex >= undecidedQueue.length - 1) {
          setCurrentIndex(Math.max(0, undecidedQueue.length - 2));
        }
      } catch {
        // error handled by mutation
      }
    },
    [currentCandidate, submitting, submitDecision, currentIndex, undecidedQueue.length],
  );

  const handleSkipAlbum = useCallback(async () => {
    if (!detail || !importRunId || submitting) return;
    setSubmitting(true);
    try {
      await api.skipReviewAlbum(importRunId, detail.album_id);
      invalidateReviewWorkflowQueries(queryClient);
      setCurrentIndex(0);
    } catch {
      // ignore
    }
    setSubmitting(false);
  }, [detail, importRunId, submitting, queryClient]);

  const handleGeneratePlan = useCallback(async () => {
    if (!importRunId) return;
    try {
      // Freeze the plan as a single atomic transaction so the commit page
      // can read the frozen summary without re-deriving from candidates.
      const plan = await api.freezeImportPlan(importRunId);
      setImportPlan(plan);
      setShowPlan(true);
      setOpenPlanAlbums(new Set());
      setPlanEditError(null);
    } catch {
      // ignore
    }
  }, [importRunId]);

  const applyPlanEdit = useCallback(
    async (edit: () => Promise<ImportPlan>) => {
      if (planEditPending) return;
      setPlanEditPending(true);
      setPlanEditError(null);
      try {
        const nextPlan = await edit();
        setImportPlan(nextPlan);
      } catch (error) {
        setPlanEditError(String(error));
      } finally {
        setPlanEditPending(false);
      }
    },
    [planEditPending],
  );

  const togglePlanAlbum = useCallback(
    (album: ImportPlanAlbumGroup) => {
      if (!importPlan) return;
      applyPlanEdit(() =>
        api.setImportPlanAlbumIncluded(importPlan.import_run_id, album.albumId, !album.included),
      );
    },
    [applyPlanEdit, importPlan],
  );

  const togglePlanImage = useCallback(
    (album: ImportPlanAlbumGroup, image: ImportPlanImage) => {
      if (!importPlan) return;
      applyPlanEdit(() =>
        api.setImportPlanImageIncluded(
          importPlan.import_run_id,
          image.image_id,
          album.albumId,
          !image.included,
        ),
      );
    },
    [applyPlanEdit, importPlan],
  );

  const movePlanImage = useCallback(
    (imageId: string, targetAlbumId: string) => {
      if (!importPlan) return;
      applyPlanEdit(() =>
        api.moveImportPlanImage(importPlan.import_run_id, imageId, targetAlbumId),
      );
    },
    [applyPlanEdit, importPlan],
  );

  const openPlanImagePreview = useCallback(
    (image: ImportPlanImage, dataUrl: string | null) => {
      setPreviewModal({ image, dataUrl });
      if (dataUrl || !importPlan) return;
      api
        .getImportPlanImagePreview(importPlan.import_run_id, image.image_id)
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
    [importPlan],
  );

  const handlePrev = useCallback(() => {
    setCurrentIndex((i) => Math.max(0, i - 1));
  }, []);

  const handleNext = useCallback(() => {
    setCurrentIndex((i) => Math.min(undecidedQueue.length - 1, i + 1));
  }, [undecidedQueue.length]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      switch (e.key) {
        case '1':
          handleDecision('keep_source');
          break;
        case '2':
          handleDecision('keep_candidate');
          break;
        case '3':
          handleDecision('keep_all');
          break;
        case '4':
          handleSkipAlbum();
          break;
        case 'ArrowLeft':
          e.preventDefault();
          handlePrev();
          break;
        case 'ArrowRight':
          e.preventDefault();
          handleNext();
          break;
        case 'o':
        case 'O':
          setOverlayMode((m) => !m);
          break;
        case 'r':
        case 'R':
          setView(DEFAULT_VIEW);
          break;
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [handleDecision, handleSkipAlbum, handlePrev, handleNext]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const handler = (e: WheelEvent) => {
      e.preventDefault();
      const factor = e.deltaY > 0 ? 0.9 : 1.1;
      setView((v) => ({
        ...v,
        scale: Math.max(0.1, Math.min(10, v.scale * factor)),
      }));
    };
    el.addEventListener('wheel', handler, { passive: false });
    return () => el.removeEventListener('wheel', handler);
  }, []);

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      if (e.button !== 0) return;
      setIsPanning(true);
      setPanStart({ x: e.clientX - view.offsetX, y: e.clientY - view.offsetY });
    },
    [view.offsetX, view.offsetY],
  );

  const handleMouseMove = useCallback(
    (e: React.MouseEvent) => {
      if (!isPanning) return;
      setView((v) => ({
        ...v,
        offsetX: e.clientX - panStart.x,
        offsetY: e.clientY - panStart.y,
      }));
    },
    [isPanning, panStart],
  );

  const handleMouseUp = useCallback(() => {
    setIsPanning(false);
  }, []);

  if (!importRunId) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="审核" description="逐一确认无法自动判断的相似图片。" />
        {runQuery.isLoading ? (
          <div className="review-loading-panel" role="status" aria-label="正在加载最近的导入任务">
            <Skeleton height={28} width="38%" />
            <Skeleton height={18} width="62%" />
          </div>
        ) : (
          <EmptyState
            title="没有可审核的导入"
            description="请先完成一次扫描，然后回来审核重复候选。"
            action={<Button onClick={() => onNavigate('scan')}>前往扫描</Button>}
          />
        )}
      </div>
    );
  }

  const progress = progressQuery.data;
  const totalCandidates = progress?.total_review_candidates ?? 0;
  const allDecided =
    (progress?.all_decided ?? false) ||
    (queueQuery.isSuccess && undecidedQueue.length === 0 && totalCandidates > 0);

  if (totalCandidates === 0 && !(showPlan && importPlan)) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="审核" description="逐一确认无法自动判断的相似图片。" />
        <EmptyState
          title="没有待审核候选"
          description="该导入任务没有需要人工确认的重复候选，可以直接生成导入计划。"
          action={
            <div className="review-empty-actions">
              <Button variant="primary" onClick={handleGeneratePlan}>
                生成导入计划
              </Button>
              <Button variant="quiet" onClick={() => onNavigate('dashboard')}>
                返回工作台
              </Button>
            </div>
          }
        />
      </div>
    );
  }

  if (showPlan && importPlan) {
    const albumGroups = planAlbumsForDisplay(importPlan);
    const keptAlbums = albumGroups.filter((album) => album.included).length;

    return (
      <div className="review-page plan-page--m3">
        <PageHeader
          title="导入计划"
          description="这是当前已冻结的入库清单；在正式提交前仍可调整，所有修改都会更新同一份 frozen plan。"
          meta={<StatusBadge tone="success">计划已冻结</StatusBadge>}
          actions={
            <>
              <Button variant="quiet" onClick={() => setShowPlan(false)}>
                返回审核
              </Button>
              <Button variant="primary" onClick={() => onNavigate('commit')}>
                前往提交确认
              </Button>
            </>
          }
        />
        <StatusBanner tone="info" title="计划与入库是两个步骤">
          此页只调整并保存计划；下一页会重新读取这份 frozen plan，再由你确认开始文件事务。
        </StatusBanner>
        <div className="import-plan-summary">
          <div className="import-plan-stats">
            <div className="plan-stat">
              <span>图集数</span>
              <strong>{importPlan.total_albums}</strong>
            </div>
            <div className="plan-stat">
              <span>图片总数</span>
              <strong>{importPlan.total_images}</strong>
            </div>
            <div className="plan-stat plan-stat--success">
              <span>计划导入</span>
              <strong>{importPlan.kept_images.length}</strong>
            </div>
            <div className="plan-stat plan-stat--warning">
              <span>计划排除</span>
              <strong>{importPlan.excluded_count}</strong>
            </div>
          </div>
          {planEditError && <div className="commit-error-msg">{planEditError}</div>}
          <div className="import-plan-kept">
            <div className="plan-list-heading">
              <div>
                <h2>图集清单</h2>
                <p>仅展开图集时加载图片行与预览，避免长清单一次渲染全部内容。</p>
              </div>
              <StatusBadge>
                {keptAlbums} 个图集 · {importPlan.kept_images.length} 张图片
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
                    onDragOver={(event) => event.preventDefault()}
                    onDrop={(event) => {
                      event.preventDefault();
                      const imageId = event.dataTransfer.getData('text/plain');
                      if (imageId) movePlanImage(imageId, album.albumId);
                    }}
                  >
                    <summary>
                      <span
                        className={`import-plan-album-title ${album.included ? '' : 'is-skipped'}`}
                      >
                        {album.albumName}
                      </span>
                      <span className="import-plan-album-meta">
                        导入 {album.imageCount} 张 / 跳过 {album.skippedImageCount} 张 ·{' '}
                        {formatFileSize(album.totalSize)}
                      </span>
                    </summary>
                    <button
                      type="button"
                      className={`plan-toggle plan-album-toggle ${album.included ? 'is-on' : 'is-off'}`}
                      disabled={planEditPending}
                      onClick={() => togglePlanAlbum(album)}
                    >
                      {album.included ? '导入' : '跳过'}
                    </button>
                    {isOpen && (
                      <div className="import-plan-image-list">
                        {album.images
                          .slice(0, planImageLimits[album.albumId] ?? PLAN_IMAGE_BATCH_SIZE)
                          .map((img) => (
                            <div
                              className={`import-plan-image-row ${img.included ? 'included' : 'skipped'}`}
                              key={img.image_id}
                              draggable
                              onDragStart={(event) => {
                                event.dataTransfer.setData('text/plain', img.image_id);
                                event.dataTransfer.effectAllowed = 'move';
                              }}
                            >
                              <PlanImageThumbnail
                                importRunId={importPlan.import_run_id}
                                image={img}
                                onOpen={openPlanImagePreview}
                              />
                              <button
                                type="button"
                                className="import-plan-image-info"
                                onClick={() => openPlanImagePreview(img, null)}
                              >
                                <span className="mono">{img.relative_path}</span>
                                <span>{formatFileSize(img.file_size)}</span>
                              </button>
                              <button
                                type="button"
                                className={`plan-toggle ${img.included ? 'is-on' : 'is-off'}`}
                                disabled={planEditPending}
                                onClick={() => togglePlanImage(album, img)}
                              >
                                {img.included ? '导入' : '跳过'}
                              </button>
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
                  onClick={() => setPlanAlbumLimit((current) => current + PLAN_ALBUM_BATCH_SIZE)}
                >
                  再显示 {PLAN_ALBUM_BATCH_SIZE} 个图集（剩余 {albumGroups.length - planAlbumLimit}{' '}
                  个）
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
      </div>
    );
  }

  if (allDecided) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="审核完成" description="人工判断已经全部保存。" />
        <EmptyState
          title="所有候选已审核"
          description={`已处理 ${progress?.decided_count ?? 0} / ${progress?.total_review_candidates ?? 0} 个候选。`}
          action={
            <Button variant="primary" onClick={handleGeneratePlan}>
              生成导入计划
            </Button>
          }
        />
      </div>
    );
  }

  const imageStyle: React.CSSProperties = {
    transform: `translate(${view.offsetX}px, ${view.offsetY}px) scale(${view.scale})`,
    transformOrigin: 'center center',
    transition: isPanning ? 'none' : 'transform var(--motion-fast) var(--ease-out)',
  };

  return (
    <div className="review-page review-page--m3" ref={containerRef}>
      <PageHeader
        title={`审核：${currentCandidate?.album_name ?? '待审核图集'}`}
        description={`当前有 ${reviewAlbumCount} 个图集包含待审核候选，可在整批分析结束前先处理。`}
        meta={
          <div className="review-header-meta">
            <StatusBadge tone="info">
              {currentIndex + 1} / {undecidedQueue.length} 个待定
            </StatusBadge>
            {detail && <StatusBadge>{formatMatchType(detail.match_type)}</StatusBadge>}
            {detail && <StatusBadge>{formatScope(detail.scope)}</StatusBadge>}
          </div>
        }
        actions={
          <>
            <Button
              variant="quiet"
              className={overlayMode ? 'is-active' : undefined}
              onClick={() => setOverlayMode((mode) => !mode)}
              title="切换叠加模式 (O)"
              aria-pressed={overlayMode}
            >
              叠加比较
            </Button>
            <Button variant="quiet" onClick={() => setView(DEFAULT_VIEW)} title="重置缩放 (R)">
              重置视图
            </Button>
          </>
        }
      />

      {detailQuery.isLoading || !detail ? (
        <div className="review-loading-panel" role="status" aria-label="正在加载候选">
          <Skeleton height={420} radius="var(--radius-image)" />
        </div>
      ) : (
        <>
          <div
            className={`review-images ${overlayMode ? 'overlay-mode' : 'side-by-side'}`}
            onMouseDown={handleMouseDown}
            onMouseMove={handleMouseMove}
            onMouseUp={handleMouseUp}
            onMouseLeave={handleMouseUp}
          >
            <div className="review-image-panel left-panel">
              <div className="review-image-label">
                <span>源图片</span>
                <kbd>1</kbd>
              </div>
              <div className="review-image-container">
                {leftPreview ? (
                  <img src={leftPreview} alt="源图片" style={imageStyle} draggable={false} />
                ) : (
                  <div className="review-image-placeholder">没有预览</div>
                )}
              </div>
            </div>
            <div className="review-image-panel right-panel">
              <div className="review-image-label">
                <span>{detail.scope === 'library' ? '历史图库匹配' : '候选图片'}</span>
                <kbd>2</kbd>
              </div>
              <div className="review-image-container">
                {overlayMode ? (
                  rightPreview ? (
                    <img
                      src={rightPreview}
                      alt="候选图片"
                      style={{
                        ...imageStyle,
                        opacity: overlayOpacity,
                        position: 'absolute',
                        top: 0,
                        left: 0,
                      }}
                      draggable={false}
                    />
                  ) : null
                ) : rightPreview ? (
                  <img src={rightPreview} alt="候选图片" style={imageStyle} draggable={false} />
                ) : (
                  <div className="review-image-placeholder">没有预览</div>
                )}
              </div>
            </div>
          </div>

          {overlayMode && (
            <div className="overlay-opacity-control">
              <label>叠加透明度: {Math.round(overlayOpacity * 100)}%</label>
              <input
                type="range"
                min="0"
                max="100"
                value={overlayOpacity * 100}
                onChange={(e) => setOverlayOpacity(Number(e.target.value) / 100)}
              />
            </div>
          )}

          <details className="review-metadata">
            <summary>查看图片与匹配详情</summary>
            <div className="review-info-grid">
              <div className="review-info-card">
                <h3>源图片</h3>
                <table>
                  <tbody>
                    <tr>
                      <td>尺寸</td>
                      <td>
                        {detail.source_image_width && detail.source_image_height
                          ? `${detail.source_image_width} x ${detail.source_image_height}`
                          : '无'}
                      </td>
                    </tr>
                    <tr>
                      <td>文件大小</td>
                      <td>{formatFileSize(detail.source_image_file_size)}</td>
                    </tr>
                    <tr>
                      <td>路径</td>
                      <td className="mono">{detail.source_image_path}</td>
                    </tr>
                  </tbody>
                </table>
              </div>
              <div className="review-info-card">
                <h3>{detail.scope === 'library' ? '历史图库匹配' : '候选图片'}</h3>
                <table>
                  <tbody>
                    <tr>
                      <td>尺寸</td>
                      <td>
                        {detail.scope === 'library'
                          ? detail.candidate_library_image_width &&
                            detail.candidate_library_image_height
                            ? `${detail.candidate_library_image_width} x ${detail.candidate_library_image_height}`
                            : '无'
                          : detail.candidate_source_image_width &&
                              detail.candidate_source_image_height
                            ? `${detail.candidate_source_image_width} x ${detail.candidate_source_image_height}`
                            : '无'}
                      </td>
                    </tr>
                    <tr>
                      <td>文件大小</td>
                      <td>
                        {detail.scope === 'library'
                          ? detail.candidate_library_image_file_size
                            ? formatFileSize(detail.candidate_library_image_file_size)
                            : '无'
                          : detail.candidate_source_image_file_size
                            ? formatFileSize(detail.candidate_source_image_file_size)
                            : '无'}
                      </td>
                    </tr>
                    <tr>
                      <td>路径</td>
                      <td className="mono">
                        {detail.candidate_source_image_path ??
                          detail.candidate_library_image_path ??
                          '无'}
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>
              <div className="review-info-card">
                <h3>匹配详情</h3>
                <table>
                  <tbody>
                    <tr>
                      <td>图集</td>
                      <td>{detail.album_name}</td>
                    </tr>
                    <tr>
                      <td>范围</td>
                      <td>{formatScope(detail.scope)}</td>
                    </tr>
                    <tr>
                      <td>匹配类型</td>
                      <td>{formatMatchType(detail.match_type)}</td>
                    </tr>
                    <tr>
                      <td>变换</td>
                      <td>{formatTransform(detail.transform_type)}</td>
                    </tr>
                    <tr>
                      <td>BLAKE3 相同</td>
                      <td>{detail.blake3_equal ? '是' : '否'}</td>
                    </tr>
                    <tr>
                      <td>像素哈希相同</td>
                      <td>{detail.pixel_hash_equal ? '是' : '否'}</td>
                    </tr>
                    <tr>
                      <td>梯度距离</td>
                      <td>{formatDistance(detail.gradient_distance)}</td>
                    </tr>
                    <tr>
                      <td>分块距离</td>
                      <td>{formatDistance(detail.block_distance)}</td>
                    </tr>
                    <tr>
                      <td>中值距离</td>
                      <td>{formatDistance(detail.median_distance)}</td>
                    </tr>
                    {detail.confidence !== null && (
                      <tr>
                        <td>置信度</td>
                        <td>{(detail.confidence * 100).toFixed(1)}%</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            </div>
          </details>

          <div className="review-actions">
            <div className="review-nav">
              <Button variant="quiet" onClick={handlePrev} disabled={currentIndex === 0}>
                ← 上一个
              </Button>
              <Button
                variant="quiet"
                onClick={handleNext}
                disabled={currentIndex >= undecidedQueue.length - 1}
              >
                下一个 →
              </Button>
            </div>
            <div className="review-decision-buttons">
              {REVIEW_DECISION_OPTIONS.map((option) => {
                const label =
                  option.decision === 'keep_candidate' && detail.scope === 'library'
                    ? '保留历史图库图片'
                    : option.label;
                return (
                  <Button
                    key={option.decision}
                    variant={option.decision === 'keep_all' ? 'secondary' : 'primary'}
                    className={option.decision === 'keep_all' ? undefined : 'review-choice'}
                    onClick={() => handleDecision(option.decision)}
                    disabled={submitting}
                    loading={submitting && submitDecision.variables?.decision === option.decision}
                    loadingLabel="正在保存…"
                    title={`${label} (${option.shortcut})`}
                  >
                    {label} <kbd>{option.shortcut}</kbd>
                  </Button>
                );
              })}
              <Button
                variant="quiet"
                className="review-skip-album"
                onClick={handleSkipAlbum}
                disabled={submitting}
                title="跳过该图集的所有候选 (4)"
              >
                跳过图集 <kbd>4</kbd>
              </Button>
            </div>
          </div>

          <p className="review-shortcuts-hint">
            <AppIcon name="review" size={16} />
            快捷键：1–4 作出决定，方向键切换，O 叠加，R 重置；滚轮缩放，拖拽平移。
          </p>
        </>
      )}
    </div>
  );
}
