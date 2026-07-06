import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { Route } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import type {
  ImportAlbumStatus,
  ImportRunDashboard,
  ScanProgress,
  ScanSourceInfo,
} from '../lib/ipc/types';

interface ScanPageProps {
  initialImportRunId?: string | null;
  onNavigate: (route: Route) => void;
}

interface ScanProgressEvent {
  state: string;
  import_run_id: string | null;
  current_stage: string;
  current_album: string | null;
  processed_images: number;
  total_albums: number;
  total_images: number;
  duplicate_count: number;
  error_count: number;
  errors: string[];
}

interface ScanDraft {
  sourcePath: string;
  sourceInfo: ScanSourceInfo | null;
}

const SCAN_DRAFT_STORAGE_KEY = 'imagedb.scan.draft';

const STAGE_LABELS: Record<string, string> = {
  idle: '空闲',
  scanning: '扫描目录',
  fingerprinting: '计算指纹',
  detecting_duplicates: '检测重复',
  completing: '完成中',
  completed: '已完成',
  cancelled: '已取消',
  failed: '失败',
  analyzing: '分析中',
  analyzed: '已分析',
  review_required: '待审核',
  ready_to_commit: '可生成入库计划',
};

const ALBUM_STATE_LABELS: Record<string, string> = {
  pending: '待分析',
  analyzing: '分析中',
  analyzed: '已分析',
  review_required: '待审核',
  failed: '失败',
};

function isScanSourceInfo(value: unknown): value is ScanSourceInfo {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<ScanSourceInfo>;
  return (
    typeof candidate.path === 'string' &&
    Array.isArray(candidate.albums) &&
    candidate.albums.every((album) => typeof album === 'string') &&
    typeof candidate.album_count === 'number'
  );
}

function loadScanDraft(): ScanDraft {
  try {
    const raw = window.localStorage.getItem(SCAN_DRAFT_STORAGE_KEY);
    if (!raw) return { sourcePath: '', sourceInfo: null };
    const parsed = JSON.parse(raw) as Partial<ScanDraft>;
    return {
      sourcePath: typeof parsed.sourcePath === 'string' ? parsed.sourcePath : '',
      sourceInfo: isScanSourceInfo(parsed.sourceInfo) ? parsed.sourceInfo : null,
    };
  } catch {
    return { sourcePath: '', sourceInfo: null };
  }
}

function saveScanDraft(draft: ScanDraft) {
  try {
    window.localStorage.setItem(SCAN_DRAFT_STORAGE_KEY, JSON.stringify(draft));
  } catch {
    // The draft is only a UI convenience; scanning itself does not depend on it.
  }
}

export function isTerminalScanState(state: string | null | undefined): boolean {
  return (
    state === 'ready_to_commit' ||
    state === 'review_required' ||
    state === 'completed' ||
    state === 'cancelled' ||
    state === 'failed'
  );
}

export function nextRouteForScanState(state: string | null | undefined): Route | null {
  if (state === 'review_required') return 'review';
  if (state === 'ready_to_commit') return 'review';
  return null;
}

export function nextActionLabelForScanState(state: string | null | undefined): string | null {
  const route = nextRouteForScanState(state);
  if (route === 'review') return '前往入库审核';
  return null;
}

function canResumeRun(run: ImportRunDashboard | null): boolean {
  if (!run) return false;
  return run.pending_albums > 0 || run.analyzing_albums > 0;
}

export function ScanPage({ initialImportRunId = null, onNavigate }: ScanPageProps) {
  const queryClient = useQueryClient();
  const [sourcePath, setSourcePath] = useState(() => loadScanDraft().sourcePath);
  const [sourceInfo, setSourceInfo] = useState<ScanSourceInfo | null>(
    () => loadScanDraft().sourceInfo,
  );
  const [activeImportRunId, setActiveImportRunId] = useState<string | null>(initialImportRunId);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [isValidating, setIsValidating] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [scanEvent, setScanEvent] = useState<ScanProgressEvent | null>(null);
  const [isScanning, setIsScanning] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const eventListenerRef = useRef<(() => void) | null>(null);

  const runsQuery = useQuery({
    queryKey: ['import-runs-dashboard'],
    queryFn: api.getImportRunsDashboard,
    refetchInterval: isScanning ? 1500 : 5000,
  });

  const activeRun = useMemo(
    () => runsQuery.data?.find((run) => run.import_run_id === activeImportRunId) ?? null,
    [activeImportRunId, runsQuery.data],
  );

  const albumsQuery = useQuery({
    queryKey: ['import-run-albums', activeImportRunId],
    queryFn: () => api.getImportRunAlbums(activeImportRunId!),
    enabled: Boolean(activeImportRunId),
    refetchInterval: isScanning ? 1500 : 5000,
  });

  const retryAlbum = useMutation({
    mutationFn: api.retryImportAlbum,
    onSuccess: async (album) => {
      setActiveImportRunId(album.import_run_id);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['import-run-albums', album.import_run_id] }),
      ]);
    },
  });

  useEffect(() => {
    if (initialImportRunId) {
      setActiveImportRunId(initialImportRunId);
      setScanEvent(null);
      setProgress(null);
      setIsScanning(false);
    }
  }, [initialImportRunId]);

  useEffect(() => {
    if (activeRun?.source_root) {
      setSourcePath(activeRun.source_root);
    }
  }, [activeRun?.source_root]);

  useEffect(() => {
    return () => {
      eventListenerRef.current?.();
    };
  }, []);

  useEffect(() => {
    saveScanDraft({ sourcePath, sourceInfo });
  }, [sourcePath, sourceInfo]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const p = await api.getScanProgress();
        if (cancelled) return;
        if (p && p.state && p.state !== 'idle' && p.import_run_id) {
          setProgress(p);
          setActiveImportRunId(p.import_run_id);
          setIsScanning(!isTerminalScanState(p.state));
        }
      } catch {
        // No scan in flight.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const attachScanListener = useCallback(async () => {
    const unlisten = await listen<ScanProgressEvent>('scan-progress', (event) => {
      setScanEvent(event.payload);
      if (event.payload.import_run_id) {
        setActiveImportRunId(event.payload.import_run_id);
      }
      if (isTerminalScanState(event.payload.state)) {
        setIsScanning(false);
      }
    });
    eventListenerRef.current?.();
    eventListenerRef.current = unlisten;
    return unlisten;
  }, []);

  const handleValidate = useCallback(async () => {
    if (!sourcePath.trim()) {
      setValidationError('请输入源目录路径');
      return;
    }
    setIsValidating(true);
    setValidationError(null);
    setSourceInfo(null);
    setActiveImportRunId(null);
    setScanEvent(null);
    setProgress(null);
    try {
      const info = await api.validateSourceDirectory(sourcePath.trim());
      setSourceInfo(info);
      if (info.album_count === 0) {
        setValidationError('未找到图集（一级子目录）。');
      }
    } catch (e) {
      setValidationError(String(e));
    } finally {
      setIsValidating(false);
    }
  }, [sourcePath]);

  const handleStartScan = useCallback(async () => {
    if (!sourceInfo || sourceInfo.album_count === 0) return;
    setScanError(null);
    setActiveImportRunId(null);
    setIsScanning(true);

    const unlisten = await attachScanListener();
    try {
      await api.startScan(sourcePath.trim());
    } catch (e) {
      setScanError(String(e));
      setIsScanning(false);
      unlisten();
    }
  }, [attachScanListener, sourceInfo, sourcePath]);

  const handleResumeScan = useCallback(async () => {
    if (!activeImportRunId) return;
    setScanError(null);
    setIsScanning(true);

    const unlisten = await attachScanListener();
    try {
      await api.resumeImportRun(activeImportRunId);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['import-run-albums', activeImportRunId] }),
      ]);
    } catch (e) {
      setScanError(String(e));
      setIsScanning(false);
      unlisten();
    }
  }, [activeImportRunId, attachScanListener, queryClient]);

  const handleCancelScan = useCallback(async () => {
    try {
      await api.cancelScan();
    } catch (e) {
      setScanError(String(e));
    }
  }, []);

  useEffect(() => {
    if (!isScanning) return;
    const interval = setInterval(async () => {
      try {
        const p = await api.getScanProgress();
        setProgress(p);
        if (p.import_run_id) setActiveImportRunId(p.import_run_id);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
          queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
          p.import_run_id
            ? queryClient.invalidateQueries({ queryKey: ['import-run-albums', p.import_run_id] })
            : Promise.resolve(),
        ]);
        if (isTerminalScanState(p.state)) {
          setIsScanning(false);
        }
      } catch {
        // ignore
      }
    }, 2000);
    return () => clearInterval(interval);
  }, [isScanning, queryClient]);

  const displayProgress = scanEvent ?? progress;
  const isFinished = isTerminalScanState(displayProgress?.state);
  const nextRoute = nextRouteForScanState(displayProgress?.state);
  const nextActionLabel = nextActionLabelForScanState(displayProgress?.state);
  const albumRows = albumsQuery.data ?? [];
  const albumCounts = activeRun
    ? {
        total: activeRun.total_albums,
        analyzed: activeRun.analyzed_albums + activeRun.review_required_albums,
        analyzing: activeRun.analyzing_albums,
        pending: activeRun.pending_albums,
        review: activeRun.review_required_albums,
        failed: activeRun.failed_albums,
      }
    : null;

  return (
    <div className="scan-page">
      <h1>新建导入</h1>

      <section className="scan-source-section">
        <h2>选择源目录</h2>
        <div className="scan-source-input">
          <input
            type="text"
            placeholder="输入源目录路径，例如 D:\\Photos\\2024"
            value={sourcePath}
            onChange={(e) => {
              setSourcePath(e.target.value);
              setSourceInfo(null);
              setValidationError(null);
              setActiveImportRunId(null);
              setScanEvent(null);
              setProgress(null);
            }}
            disabled={isScanning}
          />
          <button
            className="btn-secondary"
            onClick={handleValidate}
            disabled={isValidating || isScanning || !sourcePath.trim()}
          >
            {isValidating ? '验证中...' : '验证'}
          </button>
        </div>
        {validationError && <p className="status-error">{validationError}</p>}
        {sourceInfo && sourceInfo.album_count > 0 && (
          <div className="scan-source-info">
            <p>
              找到 <strong>{sourceInfo.album_count}</strong> 个图集：
              {sourceInfo.albums.slice(0, 5).join('、')}
              {sourceInfo.albums.length > 5 && `...等 ${sourceInfo.albums.length} 个`}
            </p>
          </div>
        )}
      </section>

      {sourceInfo && sourceInfo.album_count > 0 && !isScanning && !isFinished && (
        <section className="scan-action-section">
          <button className="btn-primary" onClick={handleStartScan}>
            开始分析
          </button>
        </section>
      )}

      {activeRun && canResumeRun(activeRun) && !isScanning && (
        <section className="scan-action-section">
          <button className="btn-primary" onClick={handleResumeScan}>
            继续分析
          </button>
          <p className="status-card-detail">
            将继续任务 {activeRun.import_run_id}，不会要求重新输入源目录。
          </p>
        </section>
      )}

      {scanError && <p className="status-error">{scanError}</p>}

      {activeRun ? (
        <section className="scan-progress-section">
          <h2>图集流程</h2>
          <div className="scan-progress-grid">
            <div className="scan-progress-card">
              <h3>总图集</h3>
              <p>{albumCounts?.total ?? 0}</p>
            </div>
            <div className="scan-progress-card">
              <h3>已分析</h3>
              <p>{albumCounts?.analyzed ?? 0}</p>
            </div>
            <div className="scan-progress-card">
              <h3>分析中</h3>
              <p>{albumCounts?.analyzing ?? 0}</p>
            </div>
            <div className="scan-progress-card">
              <h3>待分析</h3>
              <p>{albumCounts?.pending ?? 0}</p>
            </div>
            <div className="scan-progress-card">
              <h3>待审核</h3>
              <p className={(albumCounts?.review ?? 0) > 0 ? 'status-warn' : ''}>
                {albumCounts?.review ?? 0}
              </p>
            </div>
            <div className="scan-progress-card">
              <h3>失败</h3>
              <p className={(albumCounts?.failed ?? 0) > 0 ? 'status-error' : ''}>
                {albumCounts?.failed ?? 0}
              </p>
            </div>
          </div>

          <div className="table-wrapper">
            <table className="data-table">
              <thead>
                <tr>
                  <th>图集</th>
                  <th>图片</th>
                  <th>状态</th>
                  <th>重复候选</th>
                  <th>待审核</th>
                  <th>错误</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {albumRows.map((album: ImportAlbumStatus) => (
                  <tr key={album.id}>
                    <td>{album.source_name}</td>
                    <td>{album.image_count}</td>
                    <td>{ALBUM_STATE_LABELS[album.state] ?? album.state}</td>
                    <td>{album.duplicate_candidate_count}</td>
                    <td>{album.review_candidate_count}</td>
                    <td className="status-error">{album.last_error_message ?? ''}</td>
                    <td>
                      {album.state === 'failed' && (
                        <button
                          className="btn-secondary"
                          onClick={() => retryAlbum.mutate(album.id)}
                          disabled={retryAlbum.isPending}
                        >
                          重试
                        </button>
                      )}
                      {album.review_candidate_count > 0 && (
                        <button className="btn-primary" onClick={() => onNavigate('review')}>
                          审核
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
                {albumRows.length === 0 && (
                  <tr>
                    <td colSpan={7}>暂无图集状态。验证源目录后开始分析。</td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </section>
      ) : (
        <section className="scan-progress-section">
          <h2>图集流程</h2>
          <div className="table-wrapper">
            <table className="data-table">
              <tbody>
                <tr>
                  <td>暂无图集状态。验证源目录后开始分析。</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>
      )}

      {(isScanning || isFinished) && displayProgress && (
        <section className="scan-progress-section">
          <h2>分析进度</h2>

          <div className="scan-progress-grid">
            <div className="scan-progress-card">
              <h3>状态</h3>
              <p className={displayProgress.state === 'failed' ? 'status-error' : ''}>
                {STAGE_LABELS[displayProgress.current_stage] ?? displayProgress.current_stage}
              </p>
            </div>

            <div className="scan-progress-card">
              <h3>当前图集</h3>
              <p>{displayProgress.current_album ?? '-'}</p>
            </div>

            <div className="scan-progress-card">
              <h3>已处理图片</h3>
              <p>
                {displayProgress.processed_images} / {displayProgress.total_images || '?'}
              </p>
            </div>

            <div className="scan-progress-card">
              <h3>图集数</h3>
              <p>{displayProgress.total_albums}</p>
            </div>

            <div className="scan-progress-card">
              <h3>重复候选</h3>
              <p className={displayProgress.duplicate_count > 0 ? 'status-warn' : ''}>
                {displayProgress.duplicate_count}
              </p>
            </div>

            <div className="scan-progress-card">
              <h3>错误</h3>
              <p className={displayProgress.error_count > 0 ? 'status-error' : ''}>
                {displayProgress.error_count}
              </p>
            </div>
          </div>

          {isScanning && (
            <div className="scan-action-section">
              <button className="btn-danger" onClick={handleCancelScan}>
                取消扫描
              </button>
            </div>
          )}

          {displayProgress.errors.length > 0 && (
            <details className="scan-errors">
              <summary>错误详情 ({displayProgress.errors.length})</summary>
              <ul>
                {displayProgress.errors.map((err, i) => (
                  <li key={i} className="mono">
                    {err}
                  </li>
                ))}
              </ul>
            </details>
          )}

          {isFinished && (
            <div className="scan-action-section">
              {nextRoute && nextActionLabel && (
                <button className="btn-primary" onClick={() => onNavigate(nextRoute)}>
                  {nextActionLabel}
                </button>
              )}
              <button
                className="btn-secondary"
                onClick={() => {
                  setScanEvent(null);
                  setProgress(null);
                  setActiveImportRunId(null);
                  setIsScanning(false);
                }}
              >
                重置
              </button>
            </div>
          )}
        </section>
      )}
    </div>
  );
}
