import { useCallback, useEffect, useState } from 'react';
import { type QueryClient, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type { RecoveryDiagnostic, RecoveryOutcome, ReverifyResult } from '../lib/ipc/types';
import {
  Button,
  EmptyState,
  PageHeader,
  Skeleton,
  StatusBadge,
  StatusBanner,
} from '../components/ui';

interface RecoveryPageProps {
  onNavigate: (route: Route) => void;
  initialDiagnostics?: RecoveryDiagnostic[];
  enablePolling?: boolean;
}

export type RecoveryDisposition = 'recoverable' | 'conflict' | 'terminal';

export function recoveryDisposition(state: string): RecoveryDisposition {
  if (state === 'conflict') return 'conflict';
  if (state === 'failed' || state === 'cancelled') return 'terminal';
  return 'recoverable';
}

export function invalidateRecoveryWorkflowQueries(
  queryClient: Pick<QueryClient, 'invalidateQueries'>,
) {
  queryClient.invalidateQueries({ queryKey: ['recoverableTransactions'] });
  queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] });
  queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] });
}

function formatState(state: string): string {
  const map: Record<string, string> = {
    planned: '准备事务',
    staging: '复制文件',
    verifying: '验证暂存区',
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

export function RecoveryPage({
  onNavigate,
  initialDiagnostics,
  enablePolling = true,
}: RecoveryPageProps) {
  const queryClient = useQueryClient();
  const [recovering, setRecovering] = useState<string | null>(null);
  const [reverifying, setReverifying] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [lastOutcome, setLastOutcome] = useState<RecoveryOutcome | null>(null);
  const [lastReverify, setLastReverify] = useState<ReverifyResult | null>(null);

  const diagnosticsQuery = useQuery({
    queryKey: ['recoverableTransactions'],
    queryFn: api.scanRecoverableTransactions,
    initialData: initialDiagnostics,
    refetchInterval: enablePolling ? 5000 : false,
  });

  useEffect(() => {
    if (diagnosticsQuery.data) {
      setActionError(null);
    }
  }, [diagnosticsQuery.data]);

  const refresh = useCallback(() => {
    invalidateRecoveryWorkflowQueries(queryClient);
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
    <div className="recovery-page recovery-page--m3">
      <PageHeader
        title="恢复与事务处置"
        description="依据 staging、目标目录、清单和计划哈希判断下一步；证据冲突时不会自动覆盖。"
        meta={
          transactions.length > 0 ? (
            <StatusBadge tone="warning">{transactions.length} 个待处理事务</StatusBadge>
          ) : undefined
        }
        actions={
          <Button variant="quiet" onClick={refresh} disabled={recovering !== null}>
            刷新诊断
          </Button>
        }
      />

      {diagnosticsQuery.isLoading && (
        <div className="recovery-loading" role="status" aria-label="正在扫描可恢复事务">
          <Skeleton height={180} radius="var(--radius-panel)" />
        </div>
      )}
      {diagnosticsQuery.isError && (
        <StatusBanner tone="danger" title="无法读取恢复诊断">
          {String(diagnosticsQuery.error)}
        </StatusBanner>
      )}

      {actionError && (
        <StatusBanner tone="danger" title="事务操作失败">
          {actionError}
        </StatusBanner>
      )}

      {lastOutcome && (
        <StatusBanner
          tone={lastOutcome.recovered ? 'success' : lastOutcome.terminal ? 'danger' : 'warning'}
          title="恢复结果"
        >
          事务 {lastOutcome.transaction_id.slice(0, 8)}… → {formatState(lastOutcome.final_state)}。
          {lastOutcome.message}
          {!lastOutcome.recovered && lastOutcome.terminal && (
            <span> 此事务处于终态但未恢复成功，需要手动解决。</span>
          )}
        </StatusBanner>
      )}

      {lastReverify && (
        <StatusBanner tone="info" title="重新验证结果">
          事务 {lastReverify.transaction_id.slice(0, 8)}… → {lastReverify.verdict}。
          {lastReverify.message}
        </StatusBanner>
      )}

      {transactions.length === 0 && !diagnosticsQuery.isLoading ? (
        <EmptyState
          title="没有待处理事务"
          description="所有文件事务都已完成，不需要恢复或人工处置。"
          action={<Button onClick={() => onNavigate('dashboard')}>返回工作台</Button>}
        />
      ) : (
        <div className="recovery-transactions">
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
  const disposition = recoveryDisposition(tx.current_state);
  const isConflict = disposition === 'conflict';
  const isTerminalUnresolved = disposition === 'terminal';
  return (
    <article className={`recovery-card recovery-card--${disposition}`}>
      <div className="recovery-card-header">
        <div>
          <span className="recovery-card-kicker">文件事务</span>
          <h2>{tx.transaction_id.slice(0, 8)}…</h2>
        </div>
        <StatusBadge tone={isConflict || isTerminalUnresolved ? 'danger' : 'warning'}>
          {formatState(tx.current_state)}
        </StatusBadge>
      </div>

      {(isConflict || isTerminalUnresolved) && (
        <StatusBanner
          tone="danger"
          title={isConflict ? '证据冲突，已阻止自动恢复' : '事务已进入终态'}
        >
          {isConflict
            ? '请先重新验证并人工核对目录与清单；ImageDB 不会提供覆盖按钮。'
            : '此事务不能自动重启，需要保留诊断信息并人工处置。'}
        </StatusBanner>
      )}

      <div className="recovery-card-details">
        <div className="recovery-evidence">
          <h4>证据</h4>
          <dl>
            <div>
              <dt>暂存区</dt>
              <dd>{tx.staging_exists ? '存在' : '缺失'}</dd>
            </div>
            <div>
              <dt>正式目录</dt>
              <dd>{tx.target_exists ? '存在' : '缺失'}</dd>
            </div>
            <div>
              <dt>清单</dt>
              <dd>{tx.manifest_exists ? '存在' : '缺失'}</dd>
            </div>
            <div>
              <dt>计划哈希</dt>
              <dd className="mono">{tx.plan_hash ? `${tx.plan_hash.slice(0, 12)}…` : '缺失'}</dd>
            </div>
          </dl>
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
        <Button
          variant="primary"
          onClick={() => onRecover(tx.transaction_id)}
          disabled={recovering || reverifying || isConflict || isTerminalUnresolved}
          loading={recovering}
          loadingLabel="恢复中…"
          title={
            isConflict || isTerminalUnresolved ? '该事务需要人工处理，不能自动恢复' : '执行恢复'
          }
        >
          {isConflict || isTerminalUnresolved ? '自动恢复不可用' : '执行恢复'}
        </Button>
        <Button
          variant="secondary"
          onClick={() => onReverify(tx.transaction_id)}
          disabled={recovering || reverifying}
          loading={reverifying}
          loadingLabel="验证中…"
        >
          重新验证
        </Button>
        {/* No "overwrite" button — conflicts require manual resolution. */}
      </div>
    </article>
  );
}
