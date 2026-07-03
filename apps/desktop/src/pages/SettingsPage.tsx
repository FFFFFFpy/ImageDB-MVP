import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { formatDiagnostic, formatTaggedStatus, taggedStatusCode } from '../lib/format';
import { useState } from 'react';

export function SettingsPage() {
  const queryClient = useQueryClient();

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: 5000,
  });

  const [extHost, setExtHost] = useState('');
  const [extPort, setExtPort] = useState('5432');
  const [extDb, setExtDb] = useState('imagedb');
  const [extUser, setExtUser] = useState('');
  const [extPass, setExtPass] = useState('');
  const [libRoot, setLibRoot] = useState('');

  const saveSettings = useMutation({
    mutationFn: api.updateSettings,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['settings'] }),
  });

  const testExt = useMutation({
    mutationFn: () =>
      api.testExternalConnection({
        host: extHost,
        port: parseInt(extPort, 10),
        database: extDb,
        username: extUser,
        password: extPass || undefined,
      }),
  });

  const shutdown = useMutation({
    mutationFn: api.shutdownDatabase,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['database-status'] }),
  });

  return (
    <div className="settings-page">
      <h1>设置</h1>

      <section className="settings-section">
        <h2>数据库状态</h2>
        {dbStatus.data && (
          <div className="db-status-card">
            <table>
              <tbody>
                <tr>
                  <td>状态</td>
                  <td
                    className={
                      taggedStatusCode(dbStatus.data.status) === 'connected' ? 'status-ok' : ''
                    }
                  >
                    {formatTaggedStatus(dbStatus.data.status)}
                  </td>
                </tr>
                <tr>
                  <td>模式</td>
                  <td>
                    {dbStatus.data.mode
                      ? dbStatus.data.mode === 'managed_local'
                        ? '托管'
                        : '外部'
                      : '未设置'}
                  </td>
                </tr>
                <tr>
                  <td>pgvector</td>
                  <td>{dbStatus.data.pgvector_available ? '可用' : '不可用'}</td>
                </tr>
                <tr>
                  <td>迁移版本</td>
                  <td className="mono">{dbStatus.data.migration_version ?? '未执行'}</td>
                </tr>
                {dbStatus.data.managed_config && (
                  <>
                    <tr>
                      <td>数据目录</td>
                      <td className="mono">{dbStatus.data.managed_config.data_dir}</td>
                    </tr>
                    <tr>
                      <td>端口</td>
                      <td>{dbStatus.data.managed_config.port}</td>
                    </tr>
                  </>
                )}
              </tbody>
            </table>
            {dbStatus.data.diagnostics.length > 0 && (
              <details className="diagnostics">
                <summary>诊断信息 ({dbStatus.data.diagnostics.length})</summary>
                <ul>
                  {dbStatus.data.diagnostics.map((d, i) => (
                    <li key={i}>{formatDiagnostic(d)}</li>
                  ))}
                </ul>
              </details>
            )}
            <button onClick={() => shutdown.mutate()} disabled={shutdown.isPending}>
              {shutdown.isPending ? '停止中…' : '停止数据库'}
            </button>
          </div>
        )}
      </section>

      <section className="settings-section">
        <h2>外部数据库连接</h2>
        <div className="form-grid">
          <label>
            主机
            <input value={extHost} onChange={(e) => setExtHost(e.target.value)} />
          </label>
          <label>
            端口
            <input type="number" value={extPort} onChange={(e) => setExtPort(e.target.value)} />
          </label>
          <label>
            数据库名
            <input value={extDb} onChange={(e) => setExtDb(e.target.value)} />
          </label>
          <label>
            用户名
            <input value={extUser} onChange={(e) => setExtUser(e.target.value)} />
          </label>
          <label>
            密码
            <input type="password" value={extPass} onChange={(e) => setExtPass(e.target.value)} />
          </label>
        </div>
        <button onClick={() => testExt.mutate()} disabled={testExt.isPending}>
          {testExt.isPending ? '测试中…' : '测试连接'}
        </button>
        {testExt.data && (
          <div className="check-result">
            <table>
              <tbody>
                <tr>
                  <td>连接</td>
                  <td>{testExt.data.connection_ok ? '成功' : '失败'}</td>
                </tr>
                <tr>
                  <td>版本</td>
                  <td>{testExt.data.version ?? '未知'}</td>
                </tr>
                <tr>
                  <td>pgvector</td>
                  <td>{testExt.data.pgvector_available ? '可用' : '不可用'}</td>
                </tr>
                <tr>
                  <td>建表权限</td>
                  <td>{testExt.data.can_create_tables ? '有' : '无'}</td>
                </tr>
              </tbody>
            </table>
          </div>
        )}
        {testExt.isError && <pre className="status-err">{String(testExt.error)}</pre>}
      </section>

      <section className="settings-section">
        <h2>图库目录</h2>
        <label>
          目标图库根目录
          <input
            value={libRoot}
            onChange={(e) => setLibRoot(e.target.value)}
            placeholder="/path/to/library"
          />
        </label>
        <button
          onClick={() => {
            if (settings.data) {
              saveSettings.mutate({
                ...settings.data,
                library_root: libRoot || null,
              });
            }
          }}
          disabled={saveSettings.isPending}
        >
          保存
        </button>
      </section>
    </div>
  );
}
