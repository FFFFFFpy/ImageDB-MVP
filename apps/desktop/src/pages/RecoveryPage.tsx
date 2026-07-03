import { useCallback, useEffect, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type { RecoveryDiagnostic, RecoveryOutcome, ReverifyResult } from '../lib/ipc/types';

interface RecoveryPageProps {
  onNavigate: (route: Route) => void;
}

function formatState(state: string): string {
  const map: Record<string, string> = {
    planned: '准备事务',
    staging: '复制文件',
    verifying: '验证 staging',
    verified: '已验证',
    publishing: '发布目录',
    published: '已发布',
    db_committing: '数据库确认',
    library_committed: '已正式入库',
    source_archiving: '源图集归档',
    source_archived: '已完成',
    cleanup_required: '等待恢复',
    conflict: '发生冲突',
    failed: '失败',
    cancelled: '已取消',
  };
  return map[state] ?? state;
}

export function RecoveryPage({ onNavigate }: RecoveryPageProps) {
  const queryClient = useQueryClient();
  const [recovering, setRecovering] = useState<string | null>(null);
  const [reverifying, setReverifying] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [lastOutcome, setLastOutcome] = useState<RecoveryOutcome | null>(null);
  const [lastReverify, setLastReverify] = useState<ReverifyResult | null>(null);

  const diagnosticsQuery = useQuery({
    queryKey: ['recoverableTransactions'],
    queryFn: api.scanRecoverableTransactions,
    refetchInterval: 5000,
  });

  useEffect(() => {
    if (diagnosticsQuery.data) {
      setActionError(null);
    }
  }, [diagnosticsQuery.data]);

  const refresh = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['recoverableTransactions'] });
  }, [queryClient]);

  const handleRecover = useCallback(
    async (txId: string) => {
      setRecovering(txId);
      setActionError(null);
      setLastOutcome(null);
      try {
        const outcome = await api.recoverTransaction(txId);
        setLastOutcome(outcome);
        refresh();
      } catch (err) {
        setActionError(String(err));
      } finally {
        setRecovering(null);
      }
    },
    [refresh],
  );

  const handleReverify = useCallback(async (txId: string) => {
    setReverifying(txId);
    setActionError(null);
    setLastReverify(null);
    try {
      const result = await api.reverifyTransaction(txId);
      setLastReverify(result);
    } catch (err) {
      setActionError(String(err));
    } finally {
      setReverifying(null);
    }
  }, []);

  const transactions = diagnosticsQuery.data ?? [];

  return (
    <div className="recovery-page">
      <h1>恢复</h1>

      {diagnosticsQuery.isLoading && <p>正在扫描可恢复事务...</p>}
      {diagnosticsQuery.isError && (
        <div className="recovery-error">无法读取恢复诊断: {String(diagnosticsQuery.error)}</div>
      )}

      {actionError && <div className="recovery-error">{actionError}</div>}

      {lastOutcome && (
        <div className={`recovery-card ${lastOutcome.recovered ? 'success' : ''}`}>
          <h3>恢复结果</h3>
          <p>
            事务 {lastOutcome.transaction_id.slice(0, 8)}... →{' '}
            {formatState(lastOutcome.final_state)}
          </p>
          <p>{lastOutcome.message}</p>
        </div>
      )}

      {lastReverify && (
        <div className="recovery-card">
          <h3>重新验证</h3>
          <p>
            事务 {lastReverify.transaction_id.slice(0, 8)}... → {lastReverify.verdict}
          </p>
          <p>{lastReverify.message}</p>
        </div>
      )}

      {transactions.length === 0 && !diagnosticsQuery.isLoading ? (
        <div className="empty-state">
          <h1>没有可恢复事务</h1>
          <p>所有文件事务都处于已完成或终态。</p>
          <div className="recovery-actions">
            <button className="btn-secondary" onClick={refresh}>
              刷新
            </button>
            <button className="btn-primary" onClick={() => onNavigate('dashboard')}>
              返回工作台
            </button>
          </div>
        </div>
      ) : (
        <div className="recovery-transactions">
          <div className="recovery-actions">
            <button className="btn-secondary" onClick={refresh} disabled={recovering !== null}>
              刷新
            </button>
          </div>
          {transactions.map((tx) => (
            <RecoveryCard
              key={tx.transaction_id}
              tx={tx}
              recovering={recovering === tx.transaction_id}
              reverifying={reverifying === tx.transaction_id}
              onRecover={handleRecover}
              onReverify={handleReverify}
            />
          ))}
        </div>
      )}
    </div>
  );
}

interface RecoveryCardProps {
  tx: RecoveryDiagnostic;
  recovering: boolean;
  reverifying: boolean;
  onRecover: (txId: string) => void;
  onReverify: (txId: string) => void;
}

function RecoveryCard({ tx, recovering, reverifying, onRecover, onReverify }: RecoveryCardProps) {
  const isConflict = tx.current_state === 'conflict';
  return (
    <div className="recovery-card">
      <div className="recovery-card-header">
        <h3>事务 {tx.transaction_id.slice(0, 8)}...</h3>
        <span className={`state-badge state-${tx.current_state}`}>
          {formatState(tx.current_state)}
        </span>
      </div>

      <div className="recovery-card-details">
        <div className="recovery-evidence">
          <h4>证据</h4>
          <ul>
            <li className={tx.staging_exists ? 'present' : 'missing'}>
              staging: {tx.staging_exists ? '存在' : '缺失'}
            </li>
            <li className={tx.target_exists ? 'present' : 'missing'}>
              正式目录: {tx.target_exists ? '存在' : '缺失'}
            </li>
            <li className={tx.manifest_exists ? 'present' : 'missing'}>
              manifest: {tx.manifest_exists ? '存在' : '缺失'}
            </li>
            {tx.plan_hash && <li>plan hash: {tx.plan_hash.slice(0, 12)}...</li>}
          </ul>
        </div>

        <div className="recovery-diagnostics">
          <h4>诊断</h4>
          <ul>
            {tx.diagnostics.map((d, i) => (
              <li key={i}>{d}</li>
            ))}
          </ul>
          {tx.last_error && <div className="recovery-error-detail">错误: {tx.last_error}</div>}
        </div>
      </div>

      <div className="recovery-actions">
        <button
          className="btn-primary"
          onClick={() => onRecover(tx.transaction_id)}
          disabled={recovering || reverifying || isConflict}
          title={isConflict ? '冲突需要手动解决，不能自动恢复' : '执行恢复'}
        >
          {recovering ? '恢复中...' : '执行恢复'}
        </button>
        <button
          className="btn-secondary"
          onClick={() => onReverify(tx.transaction_id)}
          disabled={recovering || reverifying}
        >
          {reverifying ? '验证中...' : '重新验证'}
        </button>
        {tx.target_path && <button className="btn-secondary">打开目标目录</button>}
        {/* No "overwrite" button — conflicts require manual resolution. */}
      </div>
    </div>
  );
}
