import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { api } from '../lib/ipc/api';
import type { ScanProgress, ScanSourceInfo } from '../lib/ipc/types';
import type { Route } from '../hooks/use-router';

interface ScanPageProps {
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
  if (state === 'ready_to_commit') return 'commit';
  return null;
}

export function ScanPage({ onNavigate }: ScanPageProps) {
  const [sourcePath, setSourcePath] = useState(() => loadScanDraft().sourcePath);
  const [sourceInfo, setSourceInfo] = useState<ScanSourceInfo | null>(
    () => loadScanDraft().sourceInfo,
  );
  const [validationError, setValidationError] = useState<string | null>(null);
  const [isValidating, setIsValidating] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [scanEvent, setScanEvent] = useState<ScanProgressEvent | null>(null);
  const [isScanning, setIsScanning] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const eventListenerRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    return () => {
      eventListenerRef.current?.();
    };
  }, []);

  useEffect(() => {
    saveScanDraft({ sourcePath, sourceInfo });
  }, [sourcePath, sourceInfo]);

  // Restore an in-progress or completed scan when the page (re)mounts so
  // navigating away and back does not wipe the scan state. The scan run
  // lives in the backend; the page just needs to re-attach.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const p = await api.getScanProgress();
        if (cancelled) return;
        if (p && p.state && p.state !== 'idle' && p.import_run_id) {
          setProgress(p);
          if (isTerminalScanState(p.state)) {
            setIsScanning(false);
          } else {
            setIsScanning(true);
          }
        }
      } catch {
        // ignore — no scan in flight
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const handleValidate = useCallback(async () => {
    if (!sourcePath.trim()) {
      setValidationError('请输入源目录路径');
      return;
    }
    setIsValidating(true);
    setValidationError(null);
    setSourceInfo(null);
    try {
      const info = await api.validateSourceDirectory(sourcePath.trim());
      setSourceInfo(info);
      if (info.album_count === 0) {
        setValidationError('未找到图集（一级子目录）');
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
    setIsScanning(true);

    const unlisten = await listen<ScanProgressEvent>('scan-progress', (event) => {
      setScanEvent(event.payload);
      if (isTerminalScanState(event.payload.state)) {
        setIsScanning(false);
      }
    });
    eventListenerRef.current?.();
    eventListenerRef.current = unlisten;

    try {
      await api.startScan(sourcePath.trim());
    } catch (e) {
      setScanError(String(e));
      setIsScanning(false);
      unlisten();
    }
  }, [sourceInfo, sourcePath]);

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
      } catch {
        // ignore
      }
    }, 2000);
    return () => clearInterval(interval);
  }, [isScanning]);

  const displayProgress = scanEvent ?? progress;
  const isFinished = isTerminalScanState(displayProgress?.state);
  const nextRoute = nextRouteForScanState(displayProgress?.state);

  return (
    <div className="scan-page">
      <h1>新建导入</h1>

      <section className="scan-source-section">
        <h2>选择源目录</h2>
        <div className="scan-source-input">
          <input
            type="text"
            placeholder="输入源目录路径，例如 D:\Photos\2024"
            value={sourcePath}
            onChange={(e) => {
              setSourcePath(e.target.value);
              setSourceInfo(null);
              setValidationError(null);
            }}
            disabled={isScanning}
          />
          <button
            className="btn-secondary"
            onClick={handleValidate}
            disabled={isValidating || isScanning || !sourcePath.trim()}
          >
            {isValidating ? '验证中…' : '验证'}
          </button>
        </div>
        {validationError && <p className="status-error">{validationError}</p>}
        {sourceInfo && sourceInfo.album_count > 0 && (
          <div className="scan-source-info">
            <p>
              找到 <strong>{sourceInfo.album_count}</strong> 个图集：
              {sourceInfo.albums.slice(0, 5).join('、')}
              {sourceInfo.albums.length > 5 && `…等 ${sourceInfo.albums.length} 个`}
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

      {scanError && <p className="status-error">{scanError}</p>}

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
              {nextRoute && (
                <button className="btn-primary" onClick={() => onNavigate(nextRoute)}>
                  {nextRoute === 'review' ? '前往审核' : '前往提交'}
                </button>
              )}
              <button
                className="btn-secondary"
                onClick={() => {
                  setScanEvent(null);
                  setProgress(null);
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
