import { useQuery } from '@tanstack/react-query';
import { invoke } from '@tauri-apps/api/core';

async function getAppStatus(): Promise<string> {
  return invoke<string>('get_app_status');
}

export function App() {
  const status = useQuery({
    queryKey: ['app-status'],
    queryFn: getAppStatus,
  });

  return (
    <main className="app-shell">
      <section className="status-card">
        <p className="eyebrow">ImageDB MVP</p>
        <h1>技术探针</h1>
        <p>{status.isPending ? '正在连接 Rust Core…' : status.data}</p>
        {status.isError ? <pre>{String(status.error)}</pre> : null}
      </section>
    </main>
  );
}
