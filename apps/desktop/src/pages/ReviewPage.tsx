import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type {
  ReviewCandidateDetail,
  ReviewCandidateSummary,
  ReviewDecision,
  ImportPlan,
} from '../lib/ipc/types';

interface ReviewPageProps {
  onNavigate: (route: Route) => void;
}

interface ViewState {
  scale: number;
  offsetX: number;
  offsetY: number;
}

const DEFAULT_VIEW: ViewState = { scale: 1, offsetX: 0, offsetY: 0 };

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatDistance(val: number | null): string {
  if (val === null) return 'N/A';
  return val.toString();
}

function formatMatchType(t: string): string {
  const map: Record<string, string> = {
    file_exact: 'File Exact',
    pixel_exact: 'Pixel Exact',
    perceptual_near: 'Perceptual Near',
    perceptual_similar: 'Perceptual Similar',
  };
  return map[t] ?? t;
}

function formatScope(s: string): string {
  return s === 'intra_album' ? 'Intra-Album' : s === 'library' ? 'Library' : s;
}

function formatTransform(t: string | null): string {
  if (!t) return 'N/A';
  const map: Record<string, string> = {
    identity: 'Identity',
    rot90: 'Rotate 90',
    rot180: 'Rotate 180',
    rot270: 'Rotate 270',
    flip_h: 'Flip Horizontal',
    flip_v: 'Flip Vertical',
    transpose: 'Transpose',
    transverse: 'Transverse',
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
  const [submitting, setSubmitting] = useState(false);

  const containerRef = useRef<HTMLDivElement>(null);

  const runQuery = useQuery({
    queryKey: ['latestImportRun'],
    queryFn: () => api.getLatestCompletedImportRun(),
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

  const queue = queueQuery.data ?? [];
  const undecidedQueue = useMemo(() => queue.filter((c) => !c.has_decision), [queue]);

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
    } catch {
      // ignore
    }
  }, [importRunId]);

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
        <h1>Review</h1>
        {runQuery.isLoading ? (
          <p>Loading latest import run...</p>
        ) : (
          <div className="empty-state">
            <h1>No Completed Import</h1>
            <p>Complete a scan first, then come back to review duplicate candidates.</p>
            <button className="btn-primary" onClick={() => onNavigate('scan')}>
              Go to Scan
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
        <h1>Review</h1>
        <div className="empty-state">
          <h1>No Review Candidates</h1>
          <p>
            This import run has no uncertain duplicate candidates to review. You can proceed
            directly to the import plan.
          </p>
          <div className="toolbar" style={{ justifyContent: 'center', marginTop: '1rem' }}>
            <button className="btn-primary" onClick={handleGeneratePlan}>
              Generate Import Plan
            </button>
            <button className="btn-secondary" onClick={() => onNavigate('dashboard')}>
              Back to Dashboard
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (showPlan && importPlan) {
    return (
      <div className="review-page">
        <h1>Import Plan</h1>
        <div className="import-plan-summary">
          <div className="import-plan-stats">
            <div className="scan-progress-card">
              <h3>Albums</h3>
              <p>{importPlan.total_albums}</p>
            </div>
            <div className="scan-progress-card">
              <h3>Total Images</h3>
              <p>{importPlan.total_images}</p>
            </div>
            <div className="scan-progress-card ok">
              <h3>Kept</h3>
              <p>{importPlan.kept_images.length}</p>
            </div>
            <div className="scan-progress-card warn">
              <h3>Excluded</h3>
              <p>{importPlan.excluded_count}</p>
            </div>
          </div>
          {importPlan.skipped_albums.length > 0 && (
            <div className="import-plan-skipped">
              <h3>Skipped Albums</h3>
              <ul>
                {importPlan.skipped_albums.map((a) => (
                  <li key={a}>{a}</li>
                ))}
              </ul>
            </div>
          )}
          <div className="import-plan-kept">
            <h3>Kept Images ({importPlan.kept_images.length})</h3>
            <table className="import-plan-table">
              <thead>
                <tr>
                  <th>Album</th>
                  <th>File</th>
                  <th>Size</th>
                </tr>
              </thead>
              <tbody>
                {importPlan.kept_images.map((img) => (
                  <tr key={img.image_id}>
                    <td>{img.album_name}</td>
                    <td className="mono">{img.relative_path}</td>
                    <td>{formatFileSize(img.file_size)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
        <div className="toolbar">
          <button className="btn-primary" onClick={() => onNavigate('commit')}>
            Proceed to Commit
          </button>
          <button className="btn-secondary" onClick={() => setShowPlan(false)}>
            Back to Review
          </button>
        </div>
      </div>
    );
  }

  if (allDecided) {
    return (
      <div className="review-page">
        <h1>Review Complete</h1>
        <div className="empty-state">
          <h1>All Candidates Reviewed</h1>
          <p>
            {progress?.decided_count} of {progress?.total_review_candidates} candidates decided.
          </p>
          <div className="toolbar" style={{ justifyContent: 'center' }}>
            <button className="btn-primary" onClick={handleGeneratePlan}>
              Generate Import Plan
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
          Review{' '}
          <span className="review-counter">
            {currentIndex + 1} / {undecidedQueue.length} undecided
            {progress && ` (${progress.decided_count}/${progress.total_review_candidates} total)`}
          </span>
        </h1>
        <div className="review-header-actions">
          <button
            className={`btn-secondary ${overlayMode ? 'active' : ''}`}
            onClick={() => setOverlayMode((m) => !m)}
            title="Toggle overlay mode (O)"
          >
            Overlay
          </button>
          <button
            className="btn-secondary"
            onClick={() => setView(DEFAULT_VIEW)}
            title="Reset zoom (R)"
          >
            Reset View
          </button>
        </div>
      </div>

      {detailQuery.isLoading || !detail ? (
        <div className="review-loading">Loading candidate...</div>
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
              <div className="review-image-label">Source</div>
              <div className="review-image-container">
                {leftPreview ? (
                  <img src={leftPreview} alt="Source" style={imageStyle} draggable={false} />
                ) : (
                  <div className="review-image-placeholder">No preview</div>
                )}
              </div>
            </div>
            <div className="review-image-panel right-panel">
              <div className="review-image-label">
                {detail.scope === 'library' ? 'Library Match' : 'Candidate'}
              </div>
              <div className="review-image-container">
                {overlayMode ? (
                  rightPreview ? (
                    <img
                      src={rightPreview}
                      alt="Candidate"
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
                  <img src={rightPreview} alt="Candidate" style={imageStyle} draggable={false} />
                ) : (
                  <div className="review-image-placeholder">No preview</div>
                )}
              </div>
            </div>
          </div>

          {overlayMode && (
            <div className="overlay-opacity-control">
              <label>Overlay Opacity: {Math.round(overlayOpacity * 100)}%</label>
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
              <h3>Source Image</h3>
              <table>
                <tbody>
                  <tr>
                    <td>Dimensions</td>
                    <td>
                      {detail.source_image_width && detail.source_image_height
                        ? `${detail.source_image_width} x ${detail.source_image_height}`
                        : 'N/A'}
                    </td>
                  </tr>
                  <tr>
                    <td>File Size</td>
                    <td>{formatFileSize(detail.source_image_file_size)}</td>
                  </tr>
                  <tr>
                    <td>Path</td>
                    <td className="mono">{detail.source_image_path}</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <div className="review-info-card">
              <h3>{detail.scope === 'library' ? 'Library Match' : 'Candidate'}</h3>
              <table>
                <tbody>
                  <tr>
                    <td>Dimensions</td>
                    <td>
                      {detail.scope === 'library'
                        ? detail.candidate_library_image_width &&
                          detail.candidate_library_image_height
                          ? `${detail.candidate_library_image_width} x ${detail.candidate_library_image_height}`
                          : 'N/A'
                        : detail.candidate_source_image_width &&
                            detail.candidate_source_image_height
                          ? `${detail.candidate_source_image_width} x ${detail.candidate_source_image_height}`
                          : 'N/A'}
                    </td>
                  </tr>
                  <tr>
                    <td>File Size</td>
                    <td>
                      {detail.scope === 'library'
                        ? detail.candidate_library_image_file_size
                          ? formatFileSize(detail.candidate_library_image_file_size)
                          : 'N/A'
                        : detail.candidate_source_image_file_size
                          ? formatFileSize(detail.candidate_source_image_file_size)
                          : 'N/A'}
                    </td>
                  </tr>
                  <tr>
                    <td>Path</td>
                    <td className="mono">
                      {detail.candidate_source_image_path ??
                        detail.candidate_library_image_path ??
                        'N/A'}
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
            <div className="review-info-card">
              <h3>Match Details</h3>
              <table>
                <tbody>
                  <tr>
                    <td>Album</td>
                    <td>{detail.album_name}</td>
                  </tr>
                  <tr>
                    <td>Scope</td>
                    <td>{formatScope(detail.scope)}</td>
                  </tr>
                  <tr>
                    <td>Match Type</td>
                    <td>{formatMatchType(detail.match_type)}</td>
                  </tr>
                  <tr>
                    <td>Transform</td>
                    <td>{formatTransform(detail.transform_type)}</td>
                  </tr>
                  <tr>
                    <td>BLAKE3 Equal</td>
                    <td>{detail.blake3_equal ? 'Yes' : 'No'}</td>
                  </tr>
                  <tr>
                    <td>Pixel Hash Equal</td>
                    <td>{detail.pixel_hash_equal ? 'Yes' : 'No'}</td>
                  </tr>
                  <tr>
                    <td>Gradient Dist.</td>
                    <td>{formatDistance(detail.gradient_distance)}</td>
                  </tr>
                  <tr>
                    <td>Block Dist.</td>
                    <td>{formatDistance(detail.block_distance)}</td>
                  </tr>
                  <tr>
                    <td>Median Dist.</td>
                    <td>{formatDistance(detail.median_distance)}</td>
                  </tr>
                  {detail.confidence !== null && (
                    <tr>
                      <td>Confidence</td>
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
                Previous
              </button>
              <button
                className="btn-secondary"
                onClick={handleNext}
                disabled={currentIndex >= undecidedQueue.length - 1}
              >
                Next
              </button>
            </div>
            <div className="review-decision-buttons">
              <button
                className="btn-primary"
                onClick={() => handleDecision('keep_source')}
                disabled={submitting}
                title="Keep source image (1)"
              >
                Keep Source [1]
              </button>
              <button
                className="btn-primary"
                onClick={() => handleDecision('keep_candidate')}
                disabled={submitting}
                title="Keep candidate image (2)"
              >
                Keep {detail.scope === 'library' ? 'Library' : 'Candidate'} [2]
              </button>
              <button
                className="btn-secondary"
                onClick={() => handleDecision('keep_all')}
                disabled={submitting}
                title="Keep both images (3)"
              >
                Keep All [3]
              </button>
              <button
                className="btn-danger"
                onClick={handleSkipAlbum}
                disabled={submitting}
                title="Skip all candidates in this album (4)"
              >
                Skip Album [4]
              </button>
            </div>
          </div>

          <div className="review-shortcuts-hint">
            <span>
              Keyboard: 1-4 decide, Arrows navigate, O overlay, R reset view, Scroll zoom, Drag pan
            </span>
          </div>
        </>
      )}
    </div>
  );
}
