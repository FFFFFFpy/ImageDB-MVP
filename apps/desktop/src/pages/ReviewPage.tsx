import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type {
  ReviewCandidateDetail,
  ReviewCandidateSummary,
  ReviewDecision,
  ImportPlan,
  ImportPlanAlbum,
  ImportPlanImage,
} from '../lib/ipc/types';

interface ReviewPageProps {
  onNavigate: (route: Route) => void;
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

export function ReviewPage({ onNavigate }: ReviewPageProps) {
  const queryClient = useQueryClient();

  const [importRunId, setImportRunId] = useState<string | null>(null);
  const [currentIndex, setCurrentIndex] = useState(0);
  const [overlayMode, setOverlayMode] = useState(false);
  const [overlayOpacity, setOverlayOpacity] = useState(0.5);
  const [view, setView] = useState<ViewState>(DEFAULT_VIEW);
  const [isPanning, setIsPanning] = useState(false);
  const [panStart, setPanStart] = useState({ x: 0, y: 0 });
  const [leftPreview, setLeftPreview] = useState<string | null>(null);
  const [rightPreview, setRightPreview] = useState<string | null>(null);
  const [importPlan, setImportPlan] = useState<ImportPlan | null>(null);
  const [showPlan, setShowPlan] = useState(false);
  const [openPlanAlbums, setOpenPlanAlbums] = useState<Set<string>>(new Set());
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
    refetchInterval: 5000,
  });

  const frozenPlanQuery = useQuery({
    queryKey: ['reviewFrozenImportPlanSummary', importRunId],
    queryFn: () => api.getFrozenImportPlanSummary(importRunId!),
    enabled: !!importRunId && !showPlan && !importPlan,
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
  }, [detail?.candidate_id]);

  useEffect(() => {
    setView(DEFAULT_VIEW);
    setOverlayMode(false);
  }, [currentCandidate?.candidate_id]);

  const submitDecision = useMutation({
    mutationFn: ({ candidateId, decision }: { candidateId: string; decision: ReviewDecision }) =>
      api.submitReviewDecision(candidateId, decision),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['reviewQueue'] });
      queryClient.invalidateQueries({ queryKey: ['reviewProgress'] });
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
      queryClient.invalidateQueries({ queryKey: ['reviewQueue'] });
      queryClient.invalidateQueries({ queryKey: ['reviewProgress'] });
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
      <div className="review-page">
        <h1>审核</h1>
        {runQuery.isLoading ? (
          <p>正在加载最近的导入任务...</p>
        ) : (
          <div className="empty-state">
            <h1>没有可审核的导入</h1>
            <p>请先完成一次扫描，然后回来审核重复候选。</p>
            <button className="btn-primary" onClick={() => onNavigate('scan')}>
              前往扫描
            </button>
          </div>
        )}
      </div>
    );
  }

  const progress = progressQuery.data;
  const totalCandidates = progress?.total_review_candidates ?? 0;
  const allDecided =
    (progress?.all_decided ?? false) ||
    (queueQuery.isSuccess && undecidedQueue.length === 0 && totalCandidates > 0);

  if (totalCandidates === 0) {
    return (
      <div className="review-page">
        <h1>审核</h1>
        <div className="empty-state">
          <h1>没有待审核候选</h1>
          <p>该导入任务没有需要人工确认的重复候选，可以直接生成导入计划。</p>
          <div className="toolbar" style={{ justifyContent: 'center', marginTop: '1rem' }}>
            <button className="btn-primary" onClick={handleGeneratePlan}>
              生成导入计划
            </button>
            <button className="btn-secondary" onClick={() => onNavigate('dashboard')}>
              返回工作台
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (showPlan && importPlan) {
    const albumGroups = planAlbumsForDisplay(importPlan);
    const keptAlbums = albumGroups.filter((album) => album.included).length;

    return (
      <div className="review-page">
        <div className="import-plan-header">
          <h1>导入计划</h1>
          <div className="toolbar import-plan-actions">
            <button className="btn-primary" onClick={() => onNavigate('commit')}>
              前往提交
            </button>
            <button className="btn-secondary" onClick={() => setShowPlan(false)}>
              返回审核
            </button>
          </div>
        </div>
        <div className="import-plan-summary">
          <div className="import-plan-stats">
            <div className="scan-progress-card">
              <h3>图集数</h3>
              <p>{importPlan.total_albums}</p>
            </div>
            <div className="scan-progress-card">
              <h3>图片总数</h3>
              <p>{importPlan.total_images}</p>
            </div>
            <div className="scan-progress-card ok">
              <h3>保留</h3>
              <p>{importPlan.kept_images.length}</p>
            </div>
            <div className="scan-progress-card warn">
              <h3>排除</h3>
              <p>{importPlan.excluded_count}</p>
            </div>
          </div>
          {planEditError && <div className="commit-error-msg">{planEditError}</div>}
          <div className="import-plan-kept">
            <h3>
              导入图集 ({keptAlbums}) / 导入图片 ({importPlan.kept_images.length})
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
                      <button
                        type="button"
                        className={`plan-toggle ${album.included ? 'is-on' : 'is-off'}`}
                        disabled={planEditPending}
                        onClick={(event) => {
                          event.preventDefault();
                          event.stopPropagation();
                          togglePlanAlbum(album);
                        }}
                      >
                        {album.included ? '导入' : '跳过'}
                      </button>
                    </summary>
                    {isOpen && (
                      <div className="import-plan-image-list">
                        {album.images.map((img) => (
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
      </div>
    );
  }

  if (allDecided) {
    return (
      <div className="review-page">
        <h1>审核完成</h1>
        <div className="empty-state">
          <h1>所有候选已审核</h1>
          <p>
            已处理 {progress?.decided_count} / {progress?.total_review_candidates} 个候选。
          </p>
          <div className="toolbar" style={{ justifyContent: 'center' }}>
            <button className="btn-primary" onClick={handleGeneratePlan}>
              生成导入计划
            </button>
          </div>
        </div>
      </div>
    );
  }

  const imageStyle: React.CSSProperties = {
    transform: `translate(${view.offsetX}px, ${view.offsetY}px) scale(${view.scale})`,
    transformOrigin: 'center center',
    transition: isPanning ? 'none' : 'transform 0.1s ease-out',
  };

  return (
    <div className="review-page" ref={containerRef}>
      <div className="review-header">
        <h1>
          审核{' '}
          <span className="review-counter">
            {currentIndex + 1} / {undecidedQueue.length} 个待定
            {progress && `（总计 ${progress.decided_count}/${progress.total_review_candidates}）`}
          </span>
        </h1>
        <p className="status-card-detail">
          当前有 {reviewAlbumCount} 个图集包含待审核候选，可在整批分析结束前先处理。
        </p>
        <div className="review-header-actions">
          <button
            className={`btn-secondary ${overlayMode ? 'active' : ''}`}
            onClick={() => setOverlayMode((m) => !m)}
            title="切换叠加模式 (O)"
          >
            叠加
          </button>
          <button
            className="btn-secondary"
            onClick={() => setView(DEFAULT_VIEW)}
            title="重置缩放 (R)"
          >
            重置视图
          </button>
        </div>
      </div>

      {detailQuery.isLoading || !detail ? (
        <div className="review-loading">正在加载候选...</div>
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
              <div className="review-image-label">源图片</div>
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
                {detail.scope === 'library' ? '历史图库匹配' : '候选图片'}
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

          <div className="review-actions">
            <div className="review-nav">
              <button className="btn-secondary" onClick={handlePrev} disabled={currentIndex === 0}>
                上一个
              </button>
              <button
                className="btn-secondary"
                onClick={handleNext}
                disabled={currentIndex >= undecidedQueue.length - 1}
              >
                下一个
              </button>
            </div>
            <div className="review-decision-buttons">
              <button
                className="btn-primary"
                onClick={() => handleDecision('keep_source')}
                disabled={submitting}
                title="保留源图片 (1)"
              >
                保留源图片 [1]
              </button>
              <button
                className="btn-primary"
                onClick={() => handleDecision('keep_candidate')}
                disabled={submitting}
                title="保留候选图片 (2)"
              >
                保留{detail.scope === 'library' ? '历史图库图片' : '候选图片'} [2]
              </button>
              <button
                className="btn-secondary"
                onClick={() => handleDecision('keep_all')}
                disabled={submitting}
                title="两张都保留 (3)"
              >
                全部保留 [3]
              </button>
              <button
                className="btn-danger"
                onClick={handleSkipAlbum}
                disabled={submitting}
                title="跳过该图集的所有候选 (4)"
              >
                跳过图集 [4]
              </button>
            </div>
          </div>

          <div className="review-shortcuts-hint">
            <span>快捷键: 1-4 作出决定，方向键切换，O 叠加，R 重置视图，滚轮缩放，拖拽平移</span>
          </div>
        </>
      )}
    </div>
  );
}
