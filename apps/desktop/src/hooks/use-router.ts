import { useCallback, useEffect, useState } from 'react';

export type Route =
  | 'onboarding'
  | 'dashboard'
  | 'library'
  | 'settings'
  | 'probes'
  | 'scan'
  | 'review'
  | 'plan'
  | 'commit'
  | 'recovery';

export interface RouterState {
  route: Route;
  runId: string | null;
  fresh: boolean;
}

export interface NavigateOptions {
  runId?: string | null;
  fresh?: boolean;
  replace?: boolean;
}

function routeFromPath(path: string): Route {
  switch (path) {
    case '/onboarding':
      return 'onboarding';
    case '/settings':
      return 'settings';
    case '/library':
      return 'library';
    case '/probes':
      return 'probes';
    case '/scan':
      return 'scan';
    case '/review':
      return 'review';
    case '/plan':
      return 'plan';
    case '/commit':
      return 'commit';
    case '/recovery':
      return 'recovery';
    default:
      return 'dashboard';
  }
}

export function parseRouteHash(hash: string): RouterState {
  const raw = hash.replace(/^#/, '') || '/';
  const [path, query = ''] = raw.split('?', 2);
  const params = new URLSearchParams(query);
  return {
    route: routeFromPath(path),
    runId: params.get('runId'),
    fresh: params.get('fresh') === '1',
  };
}

function getRouterState(): RouterState {
  return parseRouteHash(window.location.hash);
}

export function useRouter() {
  const [state, setState] = useState<RouterState>(getRouterState);

  useEffect(() => {
    const handler = () => setState(getRouterState());
    window.addEventListener('hashchange', handler);
    return () => window.removeEventListener('hashchange', handler);
  }, []);

  const navigate = useCallback((to: Route, options: NavigateOptions = {}) => {
    const params = new URLSearchParams();
    if (options.runId) params.set('runId', options.runId);
    if (options.fresh) params.set('fresh', '1');
    const encodedParams = params.toString();
    const query = encodedParams ? `?${encodedParams}` : '';
    const nextHash = `#/${to}${query}`;
    if (options.replace) {
      window.history.replaceState(
        null,
        '',
        `${window.location.pathname}${window.location.search}${nextHash}`,
      );
      setState(parseRouteHash(nextHash));
      return;
    }
    window.location.hash = nextHash.slice(1);
  }, []);

  return { ...state, navigate };
}
