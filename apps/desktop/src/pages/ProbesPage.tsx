import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { useState } from 'react';
import { formatDiagnostic } from '../lib/format';
import type { DiagnosticItem } from '../lib/ipc/types';
import type {
  PostgresProbeResult,
  ImageFingerprintProbeResult,
  FileTransactionProbeResult,
  AllProbeResults,
} from '../lib/ipc/types';
import { Button, PageHeader, StatusBadge, StatusBanner } from '../components/ui';

type TabKey = 'postgres' | 'fingerprint' | 'file_tx';

function formatFileTransactionState(state: string): string {
  const map: Record<string, string> = {
    planned: '已计划',
    staging: '复制到暂存区',
    verifying: '验证中',
    verified: '已验证',
    publishing: '发布中',
    published: '已发布',
    db_committing: '数据库确认中',
    library_committed: '已正式入库',
    source_archiving: '源图集归档中',
    source_archived: '源图集已归档',
    cleanup_required: '需要清理',
    conflict: '发生冲突',
    failed: '失败',
    cancelled: '已取消',
  };
  return map[state.toLowerCase()] ?? state;
}

export function ProbesPage({ enablePolling = true }: { enablePolling?: boolean } = {}) {
  const [tab, setTab] = useState<TabKey>('postgres');
  const [pgResult, setPgResult] = useState<PostgresProbeResult | null>(null);
  const [fpResult, setFpResult] = useState<ImageFingerprintProbeResult | null>(null);
  const [ftResult, setFtResult] = useState<FileTransactionProbeResult | null>(null);

  const criticalGuardStatus = useQuery({
    queryKey: ['critical-operation-guard-status'],
    queryFn: api.getCriticalOperationGuardStatus,
    refetchInterval: enablePolling ? 1000 : false,
    retry: false,
  });

  const connectionStatus = useMutation({
    mutationFn: api.getAppStatus,
  });

  const postgresProbe = useMutation({
    mutationFn: api.probePostgres,
    onSuccess: setPgResult,
  });

  const fingerprintProbe = useMutation({
    mutationFn: api.probeFingerprint,
    onSuccess: setFpResult,
  });

  const fileTxProbe = useMutation({
    mutationFn: api.probeFileTransaction,
    onSuccess: setFtResult,
  });

  const allProbes = useMutation({
    mutationFn: api.runAllProbes,
    onSuccess(data: AllProbeResults) {
      setPgResult(data.postgres);
      setFpResult(data.fingerprint);
      setFtResult(data.file_transaction);
    },
  });

  const runAll = () => allProbes.mutate();
  const isRunning = allProbes.isPending;
  const postgresProbeLocked = criticalGuardStatus.data?.is_blocked !== false;

  return (
    <div className="probes-page probes-page--m3">
      <PageHeader
        title="技术探针"
        description="面向环境诊断的开发工具，不属于日常导入流程。运行文件事务探针会使用隔离测试目录。"
        meta={<StatusBadge tone="warning">高级诊断</StatusBadge>}
        actions={
          <Button
            variant="primary"
            onClick={runAll}
            loading={isRunning}
            loadingLabel="运行中…"
            disabled={postgresProbeLocked}
          >
            运行全部探针
          </Button>
        }
      />

      {criticalGuardStatus.isLoading && (
        <StatusBanner tone="info" title="正在确认任务状态">
          数据库探针会在安全状态确认后开放。
        </StatusBanner>
      )}
      {criticalGuardStatus.isError && (
        <StatusBanner
          tone="danger"
          title="无法确认任务状态，数据库探针已锁定"
          actions={
            <Button variant="quiet" onClick={() => void criticalGuardStatus.refetch()}>
              重试
            </Button>
          }
        >
          {String(criticalGuardStatus.error)}
        </StatusBanner>
      )}
      {criticalGuardStatus.data?.is_blocked && (
        <StatusBanner tone="warning" title="当前任务运行期间已锁定数据库探针">
          {criticalGuardStatus.data.blocking_reason ?? '请等待当前任务结束后再运行。'}
        </StatusBanner>
      )}
      {allProbes.isError && (
        <StatusBanner tone="danger" title="无法运行全部探针">
          {String(allProbes.error)}
        </StatusBanner>
      )}

      <div className="toolbar">
        <Button
          variant="secondary"
          onClick={() => connectionStatus.mutate()}
          loading={connectionStatus.isPending}
          loadingLabel="测试中…"
        >
          连接测试
        </Button>
      </div>

      {connectionStatus.data && <p className="status-ok">{connectionStatus.data}</p>}
      {connectionStatus.isError && (
        <pre className="status-err">{String(connectionStatus.error)}</pre>
      )}

      <nav className="tabs" aria-label="探针类别">
        {(['postgres', 'fingerprint', 'file_tx'] as TabKey[]).map((key) => (
          <button
            key={key}
            className={tab === key ? 'tab active' : 'tab'}
            onClick={() => setTab(key)}
            aria-pressed={tab === key}
          >
            {key === 'postgres' ? '数据库' : key === 'fingerprint' ? '图片指纹' : '文件事务'}
          </button>
        ))}
      </nav>

      {tab === 'postgres' && (
        <div className="probe-panel">
          <button
            onClick={() => postgresProbe.mutate()}
            disabled={postgresProbe.isPending || postgresProbeLocked}
          >
            {postgresProbe.isPending ? '检测中…' : '运行数据库探针'}
          </button>
          {pgResult && <PgResultView result={pgResult} />}
          {postgresProbe.isError && <pre className="status-err">{String(postgresProbe.error)}</pre>}
        </div>
      )}

      {tab === 'fingerprint' && (
        <div className="probe-panel">
          <button onClick={() => fingerprintProbe.mutate()} disabled={fingerprintProbe.isPending}>
            {fingerprintProbe.isPending ? '计算中…' : '运行指纹探针'}
          </button>
          {fpResult && <FpResultView result={fpResult} />}
          {fingerprintProbe.isError && (
            <pre className="status-err">{String(fingerprintProbe.error)}</pre>
          )}
        </div>
      )}

      {tab === 'file_tx' && (
        <div className="probe-panel">
          <button onClick={() => fileTxProbe.mutate()} disabled={fileTxProbe.isPending}>
            {fileTxProbe.isPending ? '执行中…' : '运行文件事务探针'}
          </button>
          {ftResult && <FtResultView result={ftResult} />}
          {fileTxProbe.isError && <pre className="status-err">{String(fileTxProbe.error)}</pre>}
        </div>
      )}
    </div>
  );
}

function DiagnosticsList({ items }: { items: DiagnosticItem[] }) {
  if (items.length === 0) return null;
  return (
    <details className="diagnostics">
      <summary>诊断日志 ({items.length})</summary>
      <ul>
        {items.map((item, i) => (
          <li key={i}>{formatDiagnostic(item)}</li>
        ))}
      </ul>
    </details>
  );
}

function PgResultView({ result }: { result: PostgresProbeResult }) {
  return (
    <div className="probe-result">
      <table>
        <tbody>
          <tr>
            <td>PostgreSQL 可用</td>
            <td>{result.available ? '是' : '否'}</td>
          </tr>
          <tr>
            <td>托管实例</td>
            <td>{result.managed ? '是' : '否'}</td>
          </tr>
          <tr>
            <td>pgvector</td>
            <td>{result.pgvector_available ? '可用' : '不可用'}</td>
          </tr>
          <tr>
            <td>端口</td>
            <td>{result.port ?? '-'}</td>
          </tr>
          <tr>
            <td>数据目录</td>
            <td className="mono">{result.data_dir ?? '-'}</td>
          </tr>
          <tr>
            <td>数据库已创建</td>
            <td>{result.database_created ? '是' : '否'}</td>
          </tr>
          <tr>
            <td>连接正常</td>
            <td>{result.connection_ok ? '是' : '否'}</td>
          </tr>
        </tbody>
      </table>
      <DiagnosticsList items={result.diagnostics} />
    </div>
  );
}

function FpResultView({ result }: { result: ImageFingerprintProbeResult }) {
  return (
    <div className="probe-result">
      <p>
        状态: {result.success ? '成功' : '失败'} | 指纹数量: {result.fingerprints.length}
      </p>
      {result.fingerprints.map((fp, i) => (
        <div key={i} className="fingerprint-card">
          <p className="mono">
            {fp.file_path.split(/[/\\]/).pop()} ({fp.format}, {fp.width}x{fp.height}, {fp.file_size}{' '}
            字节)
          </p>
          <table>
            <tbody>
              <tr>
                <td>指纹版本</td>
                <td className="mono">{fp.fingerprint_version}</td>
              </tr>
              <tr>
                <td>文件 BLAKE3</td>
                <td className="mono">{fp.blake3_bytes} bytes</td>
              </tr>
              <tr>
                <td>像素 BLAKE3</td>
                <td className="mono">{fp.pixel_hash_bytes} bytes</td>
              </tr>
              <tr>
                <td>BlockHash 16×16</td>
                <td className="mono">{fp.block_hash_bits} bits</td>
              </tr>
              <tr>
                <td>DoubleGradient 32×32</td>
                <td className="mono">{fp.double_gradient_hash_bits} bits</td>
              </tr>
            </tbody>
          </table>
        </div>
      ))}
      <DiagnosticsList items={result.diagnostics} />
    </div>
  );
}

function FtResultView({ result }: { result: FileTransactionProbeResult }) {
  return (
    <div className="probe-result">
      <table>
        <tbody>
          <tr>
            <td>事务 ID</td>
            <td className="mono">{result.transaction_id}</td>
          </tr>
          <tr>
            <td>状态</td>
            <td>{formatFileTransactionState(result.state)}</td>
          </tr>
          <tr>
            <td>BLAKE3 校验</td>
            <td>{result.blake3_verified ? '通过' : '未通过'}</td>
          </tr>
          <tr>
            <td>源文件</td>
            <td>{result.source_files.join(', ')}</td>
          </tr>
          <tr>
            <td>已发布文件</td>
            <td>{result.published_files.length}</td>
          </tr>
          <tr>
            <td>清单</td>
            <td className="mono">{result.manifest_path ?? '-'}</td>
          </tr>
        </tbody>
      </table>
      <DiagnosticsList items={result.diagnostics} />
    </div>
  );
}
