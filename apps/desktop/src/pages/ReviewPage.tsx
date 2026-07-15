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
  onGoCommit?: (importRunId: string) => void;
  onWorkflowAbandoned?: () => void;
  onPlanEditPendingChange?: (pending: boolean) => void;
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

export type PreviewState = 'idle' | 'loading' | 'success' | 'error';

interface PreviewEvidence {
  candidateId: string | null;
  state: PreviewState;
  dataUrl: string | null;
  error: string | null;
}

interface ReviewMutationError {
  message: string;
  retryHint: string;
  mayBeSaved: boolean;
}

function ReviewMutationErrorBanner({ error }: { error: ReviewMutationError }) {
  return (
    <StatusBanner
      tone={error.mayBeSaved ? 'warning' : 'danger'}
      title={error.mayBeSaved ? '审核操作可能已保存' : '审核操作未保存'}
    >
      {error.message} {error.retryHint}
    </StatusBanner>
  );
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

export function zoomViewAtPointer(
  view: ViewState,
  clientX: number,
  clientY: number,
  rect: Pick<DOMRect, 'left' | 'top' | 'width' | 'height'>,
  deltaY: number,
): ViewState {
  const nextScale = Math.max(0.1, Math.min(10, view.scale * (deltaY > 0 ? 0.9 : 1.1)));
  const ratio = nextScale / view.scale;
  const pointerX = clientX - (rect.left + rect.width / 2);
  const pointerY = clientY - (rect.top + rect.height / 2);
  return {
    scale: nextScale,
    offsetX: pointerX - (pointerX - view.offsetX) * ratio,
    offsetY: pointerY - (pointerY - view.offsetY) * ratio,
  };
}

export function shouldIgnoreReviewShortcut(event: KeyboardEvent, previewOpen: boolean): boolean {
  if (
    previewOpen ||
    event.defaultPrevented ||
    event.isComposing ||
    event.repeat ||
    event.ctrlKey ||
    event.altKey ||
    event.metaKey
  ) {
    return true;
  }
  const target = event.target;
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target instanceof HTMLElement &&
      (target.isContentEditable ||
        target.getAttribute('contenteditable') === 'true' ||
        target.closest('[contenteditable="true"]') !== null))
  );
}

export function useNonPassiveWheelZoom(
  ref: React.RefObject<HTMLElement | null>,
  onWheel: (event: WheelEvent) => void,
) {
  useEffect(() => {
    const element = ref.current;
    if (!element) return;

    element.addEventListener('wheel', onWheel, { passive: false });
    return () => element.removeEventListener('wheel', onWheel);
  }, [onWheel, ref]);
}

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

export async function invalidateReviewWorkflowQueries(queryClient: ReviewInvalidationClient) {
  const options = { throwOnError: true } as const;
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ['reviewQueue'] }, options),
    queryClient.invalidateQueries({ queryKey: ['reviewProgress'] }, options),
    queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }, options),
    queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }, options),
    queryClient.invalidateQueries({ queryKey: ['import-run-albums'] }, options),
  ]);
}

function useReviewPreviewEvidence(
  candidateId: string | null,
  detailReady: boolean,
  side: 'source' | 'candidate',
  initialDataUrl: string | null,
) {
  const [attempt, setAttempt] = useState(0);
  const [evidence, setEvidence] = useState<PreviewEvidence>({
    candidateId: null,
    state: 'idle',
    dataUrl: null,
    error: null,
  });

  useEffect(() => {
    let cancelled = false;
    if (!candidateId || !detailReady) {
      setEvidence({ candidateId, state: 'idle', dataUrl: null, error: null });
      return () => {
        cancelled = true;
      };
    }

    const candidateAtRequest = candidateId;
    const suppliedPreview = attempt === 0 ? initialDataUrl : null;
    setEvidence({
      candidateId: candidateAtRequest,
      state: 'loading',
      dataUrl: suppliedPreview,
      error: null,
    });

    if (suppliedPreview) {
      return () => {
        cancelled = true;
      };
    }

    api
      .getImagePreview(candidateAtRequest, side)
      .then((preview) => {
        if (cancelled) return;
        if (!preview.data_url) {
          setEvidence({
            candidateId: candidateAtRequest,
            state: 'error',
            dataUrl: null,
            error: '预览服务返回了空图片数据。',
          });
          return;
        }
        setEvidence({
          candidateId: candidateAtRequest,
          state: 'loading',
          dataUrl: preview.data_url,
          error: null,
        });
      })
      .catch((error) => {
        if (cancelled) return;
        setEvidence({
          candidateId: candidateAtRequest,
          state: 'error',
          dataUrl: null,
          error: String(error),
        });
      });

    return () => {
      cancelled = true;
    };
  }, [attempt, candidateId, detailReady, initialDataUrl, side]);

  const currentEvidence =
    evidence.candidateId === candidateId
      ? evidence
      : {
          candidateId,
          state: detailReady ? ('loading' as const) : ('idle' as const),
          dataUrl: null,
          error: null,
        };

  const markLoaded = useCallback(() => {
    setEvidence((current) =>
      current.candidateId === candidateId && current.dataUrl
        ? { ...current, state: 'success', error: null }
        : current,
    );
  }, [candidateId]);

  const markFailed = useCallback(() => {
    setEvidence((current) =>
      current.candidateId === candidateId
        ? {
            ...current,
            state: 'error',
            dataUrl: null,
            error: '图片数据无法显示。',
          }
        : current,
    );
  }, [candidateId]);

  const retry = useCallback(() => {
    if (!candidateId || !detailReady) return;
    setEvidence({ candidateId, state: 'loading', dataUrl: null, error: null });
    setAttempt((current) => current + 1);
  }, [candidateId, detailReady]);

  return { evidence: currentEvidence, markLoaded, markFailed, retry };
}

interface ReviewImageContainerProps {
  children: React.ReactNode;
  onWheelZoom: (event: WheelEvent) => void;
}

function ReviewImageContainer({ children, onWheelZoom }: ReviewImageContainerProps) {
  const ref = useRef<HTMLDivElement>(null);
  useNonPassiveWheelZoom(ref, onWheelZoom);
  return (
    <div className="review-image-container" ref={ref}>
      {children}
    </div>
  );
}

interface ReviewPreviewContentProps {
  evidence: PreviewEvidence;
  label: string;
  alt: string;
  style: React.CSSProperties;
  onLoaded: () => void;
  onFailed: () => void;
  onRetry: () => void;
}

function ReviewPreviewContent({
  evidence,
  label,
  alt,
  style,
  onLoaded,
  onFailed,
  onRetry,
}: ReviewPreviewContentProps) {
  if (evidence.state === 'idle') {
    return (
      <div className="review-image-placeholder" role="status">
        等待加载{label}预览…
      </div>
    );
  }

  if (evidence.state === 'error') {
    return (
      <div className="review-image-placeholder review-image-placeholder--error" role="alert">
        <strong>无法加载{label}预览</strong>
        <span>{evidence.error}</span>
        <Button variant="quiet" onMouseDown={(event) => event.stopPropagation()} onClick={onRetry}>
          重试{label}预览
        </Button>
      </div>
    );
  }

  if (!evidence.dataUrl) {
    return (
      <div className="review-image-placeholder" role="status">
        正在加载{label}预览…
      </div>
    );
  }

  return (
    <>
      <img
        src={evidence.dataUrl}
        alt={alt}
        style={style}
        draggable={false}
        onLoad={onLoaded}
        onError={onFailed}
      />
      {evidence.state === 'loading' && (
        <span className="review-image-loading-label" role="status">
          正在加载{label}预览…
        </span>
      )}
    </>
  );
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

function formatDistance(val: number | null, bitLength: number, ratio: number | null): string {
  if (val === null) return '无';
  const normalized = ratio ?? val / bitLength;
  return `${val} / ${bitLength}（距离 ${(normalized * 100).toFixed(1)}%）`;
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
    identity: '原方向',
    rot90: '旋转 90°',
    rot180: '旋转 180°',
    rot270: '旋转 270°',
    flip_h: '水平翻转',
    flip_v: '垂直翻转',
    transpose: '主对角线翻转',
    transverse: '副对角线翻转',
  };
  return map[t] ?? t;
}

export function ReviewPage({
  onNavigate,
  onGoCommit,
  onWorkflowAbandoned,
  onPlanEditPendingChange,
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
  const [submissionAction, setSubmissionAction] = useState<ReviewDecision | 'skip_album' | null>(
    null,
  );
  const [decisionError, setDecisionError] = useState<ReviewMutationError | null>(null);
  const submissionLockRef = useRef(false);
  const submittedCandidateIdsRef = useRef(new Set<string>());
  const skippedAlbumIdsRef = useRef(new Set<string>());
  const activeCandidateIdRef = useRef<string | null>(null);
  const initialPreviewCandidateIdRef = useRef<string | null>(null);
  const [planGenerationPending, setPlanGenerationPending] = useState(false);
  const [planGenerationError, setPlanGenerationError] = useState<string | null>(null);
  const [workflowAbandonConfirm, setWorkflowAbandonConfirm] = useState(false);
  const [workflowAbandonPending, setWorkflowAbandonPending] = useState(false);
  const [workflowAbandonError, setWorkflowAbandonError] = useState<string | null>(null);

  useEffect(() => {
    onPlanEditPendingChange?.(planEditPending || planGenerationPending || workflowAbandonPending);
  }, [onPlanEditPendingChange, planEditPending, planGenerationPending, workflowAbandonPending]);

  useEffect(
    () => () => {
      onPlanEditPendingChange?.(false);
    },
    [onPlanEditPendingChange],
  );

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

  const currentCandidateId = currentCandidate?.candidate_id ?? null;
  activeCandidateIdRef.current = currentCandidateId;
  if (initialPreviews && currentCandidateId && !initialPreviewCandidateIdRef.current) {
    initialPreviewCandidateIdRef.current = currentCandidateId;
  }
  const initialPreviewsMatchCurrent =
    initialPreviewCandidateIdRef.current === currentCandidateId ? initialPreviews : null;
  const detail = detailQuery.data?.candidate_id === currentCandidateId ? detailQuery.data : null;
  const detailCandidateMismatch =
    detailQuery.isSuccess &&
    detailQuery.data !== undefined &&
    detailQuery.data.candidate_id !== currentCandidateId;
  const detailReady = detailQuery.isSuccess && detail !== null && currentCandidateId !== null;
  const sourcePreview = useReviewPreviewEvidence(
    currentCandidateId,
    detailReady,
    'source',
    initialPreviewsMatchCurrent?.left ?? null,
  );
  const candidatePreview = useReviewPreviewEvidence(
    currentCandidateId,
    detailReady,
    'candidate',
    initialPreviewsMatchCurrent?.right ?? null,
  );
  const currentCandidateAlreadySubmitted = currentCandidateId
    ? submittedCandidateIdsRef.current.has(currentCandidateId)
    : false;
  const currentAlbumAlreadySkipped = detail
    ? skippedAlbumIdsRef.current.has(detail.album_id)
    : false;
  const decisionReady =
    detailReady &&
    sourcePreview.evidence.state === 'success' &&
    candidatePreview.evidence.state === 'success' &&
    !currentCandidateAlreadySubmitted &&
    !currentAlbumAlreadySkipped &&
    !submitting;
  const skipAlbumReady =
    detailReady && !currentCandidateAlreadySubmitted && !currentAlbumAlreadySkipped && !submitting;

  useEffect(() => {
    setView(DEFAULT_VIEW);
    setOverlayMode(false);
    setDecisionError(null);
  }, [currentCandidateId]);

  useEffect(() => {
    setCurrentIndex((current) => Math.max(0, Math.min(current, undecidedQueue.length - 1)));
  }, [undecidedQueue.length]);

  const submitDecision = useMutation({
    mutationFn: ({ candidateId, decision }: { candidateId: string; decision: ReviewDecision }) =>
      api.submitReviewDecision(candidateId, decision),
  });

  const handleDecision = useCallback(
    async (decision: ReviewDecision) => {
      if (!currentCandidate || !decisionReady || submissionLockRef.current) return;
      const candidateIdAtSubmit = currentCandidate.candidate_id;
      submissionLockRef.current = true;
      setSubmitting(true);
      setSubmissionAction(decision);
      setDecisionError(null);
      let decisionSaved = false;
      try {
        await submitDecision.mutateAsync({
          candidateId: currentCandidate.candidate_id,
          decision,
        });
        decisionSaved = true;
        submittedCandidateIdsRef.current.add(currentCandidate.candidate_id);
        await invalidateReviewWorkflowQueries(queryClient);
        if (currentIndex >= undecidedQueue.length - 1) {
          setCurrentIndex(Math.max(0, undecidedQueue.length - 2));
        }
      } catch (error) {
        if (activeCandidateIdRef.current === candidateIdAtSubmit) {
          setDecisionError({
            message: String(error),
            retryHint: decisionSaved
              ? '审核决定可能已经保存，但队列刷新失败。请重新加载审核页确认，不要重复提交。'
              : '决定没有从当前页面移除。请重新点击刚才的审核决定重试。',
            mayBeSaved: decisionSaved,
          });
        }
      } finally {
        submissionLockRef.current = false;
        setSubmitting(false);
        setSubmissionAction(null);
      }
    },
    [
      currentCandidate,
      currentIndex,
      decisionReady,
      queryClient,
      submitDecision,
      undecidedQueue.length,
    ],
  );

  const handleSkipAlbum = useCallback(async () => {
    if (
      !detail ||
      !currentCandidateId ||
      !importRunId ||
      !skipAlbumReady ||
      submissionLockRef.current
    )
      return;
    const candidateIdAtSubmit = currentCandidateId;
    submissionLockRef.current = true;
    setSubmitting(true);
    setSubmissionAction('skip_album');
    setDecisionError(null);
    let albumSkipped = false;
    try {
      await api.skipReviewAlbum(importRunId, detail.album_id);
      albumSkipped = true;
      skippedAlbumIdsRef.current.add(detail.album_id);
      await invalidateReviewWorkflowQueries(queryClient);
      setCurrentIndex(0);
    } catch (error) {
      if (activeCandidateIdRef.current === candidateIdAtSubmit) {
        setDecisionError({
          message: String(error),
          retryHint: albumSkipped
            ? '跳过请求可能已经保存，但队列刷新失败。请重新加载审核页确认，不要重复提交。'
            : '图集仍保留在审核队列中。请再次点击“跳过图集”重试。',
          mayBeSaved: albumSkipped,
        });
      }
    } finally {
      submissionLockRef.current = false;
      setSubmitting(false);
      setSubmissionAction(null);
    }
  }, [currentCandidateId, detail, importRunId, queryClient, skipAlbumReady]);

  const handleGeneratePlan = useCallback(async () => {
    if (!importRunId || planGenerationPending) return;
    setPlanGenerationPending(true);
    setPlanGenerationError(null);
    try {
      // Freeze the plan as a single atomic transaction so the commit page
      // can read the frozen summary without re-deriving from candidates.
      const plan = await api.freezeImportPlan(importRunId);
      setImportPlan(plan);
      setShowPlan(true);
      setOpenPlanAlbums(new Set());
      setPlanEditError(null);
      queryClient.setQueryData(['reviewFrozenImportPlanSummary', importRunId], plan);
      queryClient.setQueryData(['frozenImportPlanSummary', importRunId], plan);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
      ]);
    } catch (error) {
      setPlanGenerationError(String(error));
    } finally {
      setPlanGenerationPending(false);
    }
  }, [importRunId, planGenerationPending, queryClient]);

  const applyPlanEdit = useCallback(
    async (edit: () => Promise<ImportPlan>) => {
      if (planEditPending) return;
      setPlanEditPending(true);
      setPlanEditError(null);
      try {
        const nextPlan = await edit();
        setImportPlan(nextPlan);
        queryClient.setQueryData(
          ['reviewFrozenImportPlanSummary', nextPlan.import_run_id],
          nextPlan,
        );
        queryClient.setQueryData(['frozenImportPlanSummary', nextPlan.import_run_id], nextPlan);
        await Promise.all([
          queryClient.invalidateQueries({
            queryKey: ['reviewFrozenImportPlanSummary', nextPlan.import_run_id],
          }),
          queryClient.invalidateQueries({
            queryKey: ['frozenImportPlanSummary', nextPlan.import_run_id],
          }),
          queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
          queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
        ]);
      } catch (error) {
        setPlanEditError(String(error));
      } finally {
        setPlanEditPending(false);
      }
    },
    [planEditPending, queryClient],
  );

  const handleAbandonWorkflow = useCallback(async () => {
    if (!importPlan || planEditPending || workflowAbandonPending) return;
    const runId = importPlan.import_run_id;
    setWorkflowAbandonPending(true);
    setWorkflowAbandonError(null);
    try {
      await api.abandonFrozenImportWorkflow(runId);
      queryClient.setQueryData(['reviewFrozenImportPlanSummary', runId], null);
      queryClient.setQueryData(['frozenImportPlanSummary', runId], null);
      setImportPlan(null);
      setShowPlan(false);
      setWorkflowAbandonConfirm(false);
      setOpenPlanAlbums(new Set());
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['reviewFrozenImportPlanSummary', runId] }),
        queryClient.invalidateQueries({ queryKey: ['frozenImportPlanSummary', runId] }),
        queryClient.invalidateQueries({ queryKey: ['reviewQueue', runId] }),
        queryClient.invalidateQueries({ queryKey: ['reviewProgress', runId] }),
        queryClient.invalidateQueries({ queryKey: ['latestReviewableImportRun'] }),
        queryClient.invalidateQueries({ queryKey: ['latestCommittableImportRun'] }),
        queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
      ]);
      if (onWorkflowAbandoned) onWorkflowAbandoned();
      else onNavigate('dashboard');
    } catch (error) {
      setWorkflowAbandonError(String(error));
    } finally {
      setWorkflowAbandonPending(false);
    }
  }, [
    importPlan,
    onNavigate,
    onWorkflowAbandoned,
    planEditPending,
    queryClient,
    workflowAbandonPending,
  ]);

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
    if (submissionLockRef.current) return;
    setCurrentIndex((i) => Math.max(0, i - 1));
  }, []);

  const handleNext = useCallback(() => {
    if (submissionLockRef.current) return;
    setCurrentIndex((i) => Math.min(undecidedQueue.length - 1, i + 1));
  }, [undecidedQueue.length]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (shouldIgnoreReviewShortcut(e, previewModal !== null)) return;
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
  }, [handleDecision, handleSkipAlbum, handlePrev, handleNext, previewModal]);

  const handleWheel = useCallback((event: WheelEvent) => {
    event.preventDefault();
    const element = event.currentTarget;
    if (!(element instanceof HTMLElement)) return;
    const rect = element.getBoundingClientRect();
    setView((current) =>
      zoomViewAtPointer(current, event.clientX, event.clientY, rect, event.deltaY),
    );
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
        ) : runQuery.isError ? (
          <StatusBanner tone="danger" title="无法查询可审核任务">
            {String(runQuery.error)}
          </StatusBanner>
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
              <Button
                variant="danger"
                disabled={planEditPending || workflowAbandonPending}
                onClick={() => setWorkflowAbandonConfirm(true)}
              >
                撤销这次导入
              </Button>
              <Button
                variant="quiet"
                disabled={planEditPending || workflowAbandonPending}
                onClick={() => setShowPlan(false)}
              >
                返回审核
              </Button>
              <Button
                variant="primary"
                disabled={planEditPending || workflowAbandonPending || workflowAbandonConfirm}
                loading={planEditPending}
                loadingLabel="正在保存计划…"
                onClick={() =>
                  onGoCommit ? onGoCommit(importPlan.import_run_id) : onNavigate('commit')
                }
              >
                前往提交确认
              </Button>
            </>
          }
        />
        <StatusBanner tone="info" title="计划与入库是两个步骤">
          此页只调整并保存计划；下一页会重新读取这份 frozen plan，再由你确认开始文件事务。
        </StatusBanner>
        {workflowAbandonConfirm && (
          <StatusBanner
            tone="warning"
            title="确认撤销这次导入任务？"
            actions={
              <>
                <Button
                  variant="danger"
                  loading={workflowAbandonPending}
                  loadingLabel="正在撤销任务…"
                  onClick={handleAbandonWorkflow}
                >
                  撤销并返回工作台
                </Button>
                <Button
                  variant="quiet"
                  disabled={workflowAbandonPending}
                  onClick={() => setWorkflowAbandonConfirm(false)}
                >
                  继续保留任务
                </Button>
              </>
            }
          >
            这会结束当前导入任务并回到可新建导入的状态。已经完成的扫描、审核和计划将不再继续；源图片和图库内容不会被删除，任务记录仍会保留用于审计。
          </StatusBanner>
        )}
        {workflowAbandonError && (
          <StatusBanner tone="danger" title="撤销导入任务失败">
            {workflowAbandonError}
          </StatusBanner>
        )}
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
                      disabled={planEditPending || workflowAbandonPending || workflowAbandonConfirm}
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
                                disabled={
                                  planEditPending ||
                                  workflowAbandonPending ||
                                  workflowAbandonConfirm
                                }
                                onClick={() => togglePlanImage(album, img)}
                              >
                                {img.included ? '导入' : '跳过'}
                              </button>
                            </div>
                          ))}
                        {album.images.length >
                          (planImageLimits[album.albumId] ?? PLAN_IMAGE_BATCH_SIZE) && (
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

  const reviewQueriesLoading =
    progressQuery.isLoading || queueQuery.isLoading || frozenPlanQuery.isLoading;
  const reviewQueryError = progressQuery.error ?? queueQuery.error ?? frozenPlanQuery.error;

  if (reviewQueryError) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="审核" description="逐一确认无法自动判断的相似图片。" />
        {decisionError && <ReviewMutationErrorBanner error={decisionError} />}
        <StatusBanner
          tone="danger"
          title="无法加载审核数据"
          actions={
            <Button
              variant="secondary"
              loading={
                queueQuery.isFetching || progressQuery.isFetching || frozenPlanQuery.isFetching
              }
              loadingLabel="正在重新加载…"
              onClick={() =>
                void Promise.all([
                  queueQuery.refetch(),
                  progressQuery.refetch(),
                  frozenPlanQuery.refetch(),
                ])
              }
            >
              重新加载审核数据
            </Button>
          }
        >
          {String(reviewQueryError)}
        </StatusBanner>
      </div>
    );
  }

  if (reviewQueriesLoading) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="审核" description="逐一确认无法自动判断的相似图片。" />
        <div className="review-loading-panel" role="status" aria-label="正在加载审核数据">
          <Skeleton height={420} radius="var(--radius-image)" />
        </div>
      </div>
    );
  }

  if (totalCandidates === 0) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="审核" description="逐一确认无法自动判断的相似图片。" />
        <EmptyState
          title="没有待审核候选"
          description="该导入任务没有需要人工确认的重复候选，可以直接生成导入计划。"
          action={
            <div className="review-empty-actions">
              <Button
                variant="primary"
                onClick={handleGeneratePlan}
                loading={planGenerationPending}
                loadingLabel="正在生成…"
              >
                生成导入计划
              </Button>
              <Button
                variant="quiet"
                disabled={planGenerationPending}
                onClick={() => onNavigate('dashboard')}
              >
                返回工作台
              </Button>
            </div>
          }
        />
        {planGenerationError && (
          <StatusBanner tone="danger" title="生成导入计划失败">
            {planGenerationError}
          </StatusBanner>
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
            <Button
              variant="primary"
              onClick={handleGeneratePlan}
              loading={planGenerationPending}
              loadingLabel="正在生成…"
            >
              生成导入计划
            </Button>
          }
        />
        {planGenerationError && (
          <StatusBanner tone="danger" title="生成导入计划失败">
            {planGenerationError}
          </StatusBanner>
        )}
      </div>
    );
  }

  const imageStyle: React.CSSProperties = {
    transform: `translate(${view.offsetX}px, ${view.offsetY}px) scale(${view.scale})`,
    transformOrigin: 'center center',
    transition: isPanning ? 'none' : 'transform var(--motion-fast) var(--ease-out)',
  };

  return (
    <div className="review-page review-page--m3">
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
              disabled={!detailReady}
              onClick={() => setOverlayMode((mode) => !mode)}
              title="切换叠加模式 (O)"
              aria-pressed={overlayMode}
            >
              叠加比较
            </Button>
            <Button
              variant="quiet"
              disabled={!detailReady}
              onClick={() => setView(DEFAULT_VIEW)}
              title="重置缩放 (R)"
            >
              重置视图
            </Button>
          </>
        }
      />

      {decisionError && <ReviewMutationErrorBanner error={decisionError} />}

      {detailQuery.isError ? (
        <StatusBanner
          tone="danger"
          title="无法加载当前候选详情"
          actions={
            <Button
              variant="secondary"
              loading={detailQuery.isFetching}
              loadingLabel="正在重新加载…"
              onClick={() => void detailQuery.refetch()}
            >
              重新加载详情
            </Button>
          }
        >
          {String(detailQuery.error)}
        </StatusBanner>
      ) : detailCandidateMismatch ? (
        <StatusBanner
          tone="danger"
          title="候选详情与当前审核项不匹配"
          actions={
            <Button variant="secondary" onClick={() => void detailQuery.refetch()}>
              重新加载详情
            </Button>
          }
        >
          为避免误审，当前详情已被拒绝使用。请重新加载后再作决定。
        </StatusBanner>
      ) : detailQuery.isLoading || !detail ? (
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
              <ReviewImageContainer onWheelZoom={handleWheel}>
                <ReviewPreviewContent
                  evidence={sourcePreview.evidence}
                  label="源图片"
                  alt="源图片"
                  style={imageStyle}
                  onLoaded={sourcePreview.markLoaded}
                  onFailed={sourcePreview.markFailed}
                  onRetry={sourcePreview.retry}
                />
              </ReviewImageContainer>
            </div>
            <div className="review-image-panel right-panel">
              <div className="review-image-label">
                <span>{detail.scope === 'library' ? '历史图库匹配' : '候选图片'}</span>
                <kbd>2</kbd>
              </div>
              <ReviewImageContainer onWheelZoom={handleWheel}>
                <ReviewPreviewContent
                  evidence={candidatePreview.evidence}
                  label={detail.scope === 'library' ? '历史图库图片' : '候选图片'}
                  alt="候选图片"
                  style={
                    overlayMode
                      ? {
                          ...imageStyle,
                          opacity: overlayOpacity,
                          position: 'absolute',
                          top: 0,
                          left: 0,
                        }
                      : imageStyle
                  }
                  onLoaded={candidatePreview.markLoaded}
                  onFailed={candidatePreview.markFailed}
                  onRetry={candidatePreview.retry}
                />
              </ReviewImageContainer>
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
                      <td>BlockHash 距离</td>
                      <td>
                        {formatDistance(detail.block_distance, 256, detail.block_distance_ratio)}
                      </td>
                    </tr>
                    <tr>
                      <td>DoubleGradient 距离</td>
                      <td>
                        {formatDistance(
                          detail.double_gradient_distance,
                          544,
                          detail.double_gradient_distance_ratio,
                        )}
                      </td>
                    </tr>
                    {detail.confidence !== null && (
                      <tr>
                        <td>综合相似度</td>
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
              <Button
                variant="quiet"
                onClick={handlePrev}
                disabled={currentIndex === 0 || submitting}
              >
                ← 上一个
              </Button>
              <Button
                variant="quiet"
                onClick={handleNext}
                disabled={currentIndex >= undecidedQueue.length - 1 || submitting}
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
                    disabled={!decisionReady}
                    loading={submitting && submissionAction === option.decision}
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
                disabled={!skipAlbumReady}
                loading={submitting && submissionAction === 'skip_album'}
                loadingLabel="正在跳过图集…"
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
