import { useMutation } from '@tanstack/react-query';
import { invoke } from '@tauri-apps/api/core';
import { useState } from 'react';

interface PostgresProbeResult {
  available: boolean;
  managed: boolean;
  pgvector_available: boolean;
  port: number | null;
  data_dir: string | null;
  database_created: boolean;
  connection_ok: boolean;
  diagnostics: string[];
}

interface ImageFingerprintEntry {
  fingerprint_version: number;
  file_path: string;
  format: string;
  width: number;
  height: number;
  file_size: number;
  blake3: string;
  pixel_hash: string;
  gradient_hash: string;
  block_hash: string;
  median_hash: string;
}

interface ImageFingerprintProbeResult {
  fingerprints: ImageFingerprintEntry[];
  diagnostics: string[];
  success: boolean;
}

interface FileTransactionProbeResult {
  transaction_id: string;
  state: string;
  source_files: string[];
  published_files: string[];
  blake3_verified: boolean;
  manifest_path: string | null;
  diagnostics: string[];
}

interface AllProbeResults {
  postgres: PostgresProbeResult;
  fingerprint: ImageFingerprintProbeResult;
  file_transaction: FileTransactionProbeResult;
}

type TabKey = 'postgres' | 'fingerprint' | 'file_tx';

export function App() {
  const [tab, setTab] = useState<TabKey>('postgres');
  const [pgResult, setPgResult] = useState<PostgresProbeResult | null>(null);
  const [fpResult, setFpResult] = useState<ImageFingerprintProbeResult | null>(null);
  const [ftResult, setFtResult] = useState<FileTransactionProbeResult | null>(null);

  const connectionStatus = useMutation({
    mutationFn: () => invoke<string>('get_app_status'),
  });

  const postgresProbe = useMutation({
    mutationFn: () => invoke<PostgresProbeResult>('probe_postgres'),
    onSuccess: setPgResult,
  });

  const fingerprintProbe = useMutation({
    mutationFn: () => invoke<ImageFingerprintProbeResult>('probe_image_fingerprint'),
    onSuccess: setFpResult,
  });

  const fileTxProbe = useMutation({
    mutationFn: () => invoke<FileTransactionProbeResult>('probe_file_transaction'),
    onSuccess: setFtResult,
  });

  const allProbes = useMutation({
    mutationFn: () => invoke<AllProbeResults>('run_all_probes'),
    onSuccess(data) {
      setPgResult(data.postgres);
      setFpResult(data.fingerprint);
      setFtResult(data.file_transaction);
    },
  });

  const runAll = () => allProbes.mutate();
  const isRunning = allProbes.isPending;

  return (
    <main className="app-shell">
      <section className="status-card">
        <p className="eyebrow">ImageDB MVP</p>
        <h1>技术探针 - Milestone 0</h1>

        <div className="toolbar">
          <button onClick={() => connectionStatus.mutate()} disabled={connectionStatus.isPending}>
            连接测试
          </button>
          <button onClick={runAll} disabled={isRunning}>
            {isRunning ? '运行中…' : '运行全部探针'}
          </button>
        </div>

        {connectionStatus.data && <p className="status-ok">{connectionStatus.data}</p>}
        {connectionStatus.isError && (
          <pre className="status-err">{String(connectionStatus.error)}</pre>
        )}

        <nav className="tabs">
          {(['postgres', 'fingerprint', 'file_tx'] as TabKey[]).map((key) => (
            <button
              key={key}
              className={tab === key ? 'tab active' : 'tab'}
              onClick={() => setTab(key)}
            >
              {key === 'postgres' ? '数据库' : key === 'fingerprint' ? '图片指纹' : '文件事务'}
            </button>
          ))}
        </nav>

        {tab === 'postgres' && (
          <div className="probe-panel">
            <button onClick={() => postgresProbe.mutate()} disabled={postgresProbe.isPending}>
              {postgresProbe.isPending ? '检测中…' : '运行数据库探针'}
            </button>

            {pgResult && (
              <div className="probe-result">
                <table>
                  <tbody>
                    <tr>
                      <td>PostgreSQL 可用</td>
                      <td>{pgResult.available ? '是' : '否'}</td>
                    </tr>
                    <tr>
                      <td>托管实例</td>
                      <td>{pgResult.managed ? '是' : '否'}</td>
                    </tr>
                    <tr>
                      <td>pgvector</td>
                      <td>{pgResult.pgvector_available ? '可用' : '不可用'}</td>
                    </tr>
                    <tr>
                      <td>端口</td>
                      <td>{pgResult.port ?? '-'}</td>
                    </tr>
                    <tr>
                      <td>数据目录</td>
                      <td className="mono">{pgResult.data_dir ?? '-'}</td>
                    </tr>
                    <tr>
                      <td>数据库已创建</td>
                      <td>{pgResult.database_created ? '是' : '否'}</td>
                    </tr>
                    <tr>
                      <td>连接正常</td>
                      <td>{pgResult.connection_ok ? '是' : '否'}</td>
                    </tr>
                  </tbody>
                </table>
                <DiagnosticsList items={pgResult.diagnostics} />
              </div>
            )}
            {postgresProbe.isError && (
              <pre className="status-err">{String(postgresProbe.error)}</pre>
            )}
          </div>
        )}

        {tab === 'fingerprint' && (
          <div className="probe-panel">
            <button onClick={() => fingerprintProbe.mutate()} disabled={fingerprintProbe.isPending}>
              {fingerprintProbe.isPending ? '计算中…' : '运行指纹探针'}
            </button>

            {fpResult && (
              <div className="probe-result">
                <p>
                  状态: {fpResult.success ? '成功' : '失败'} | 指纹数量:{' '}
                  {fpResult.fingerprints.length}
                </p>

                {fpResult.fingerprints.map((fp, i) => (
                  <div key={i} className="fingerprint-card">
                    <p className="mono">
                      {fp.file_path.split(/[/\\]/).pop()} ({fp.format}, {fp.width}x{fp.height},{' '}
                      {fp.file_size} bytes)
                    </p>
                    <table>
                      <tbody>
                        <tr>
                          <td>指纹版本</td>
                          <td className="mono">{fp.fingerprint_version}</td>
                        </tr>
                        <tr>
                          <td>BLAKE3</td>
                          <td className="mono">{fp.blake3.slice(0, 32)}...</td>
                        </tr>
                        <tr>
                          <td>Pixel Hash</td>
                          <td className="mono">{fp.pixel_hash}</td>
                        </tr>
                        <tr>
                          <td>Gradient Hash</td>
                          <td className="mono">{fp.gradient_hash}</td>
                        </tr>
                        <tr>
                          <td>Block Hash</td>
                          <td className="mono">{fp.block_hash}</td>
                        </tr>
                        <tr>
                          <td>Median Hash</td>
                          <td className="mono">{fp.median_hash}</td>
                        </tr>
                      </tbody>
                    </table>
                  </div>
                ))}

                <DiagnosticsList items={fpResult.diagnostics} />
              </div>
            )}
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

            {ftResult && (
              <div className="probe-result">
                <table>
                  <tbody>
                    <tr>
                      <td>事务 ID</td>
                      <td className="mono">{ftResult.transaction_id}</td>
                    </tr>
                    <tr>
                      <td>状态</td>
                      <td>{ftResult.state}</td>
                    </tr>
                    <tr>
                      <td>BLAKE3 校验</td>
                      <td>{ftResult.blake3_verified ? '通过' : '未通过'}</td>
                    </tr>
                    <tr>
                      <td>源文件</td>
                      <td>{ftResult.source_files.join(', ')}</td>
                    </tr>
                    <tr>
                      <td>已发布文件</td>
                      <td>{ftResult.published_files.length}</td>
                    </tr>
                    <tr>
                      <td>Manifest</td>
                      <td className="mono">{ftResult.manifest_path ?? '-'}</td>
                    </tr>
                  </tbody>
                </table>
                <DiagnosticsList items={ftResult.diagnostics} />
              </div>
            )}
            {fileTxProbe.isError && <pre className="status-err">{String(fileTxProbe.error)}</pre>}
          </div>
        )}
      </section>
    </main>
  );
}

function DiagnosticsList({ items }: { items: string[] }) {
  if (items.length === 0) return null;
  return (
    <details className="diagnostics">
      <summary>诊断日志 ({items.length})</summary>
      <ul>
        {items.map((item, i) => (
          <li key={i}>{item}</li>
        ))}
      </ul>
    </details>
  );
}
