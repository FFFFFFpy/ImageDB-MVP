import type { SVGProps } from 'react';

export type AppIconName =
  'brand' | 'dashboard' | 'import' | 'review' | 'commit' | 'recovery' | 'settings' | 'arrow';

interface AppIconProps extends SVGProps<SVGSVGElement> {
  name: AppIconName;
  size?: number;
}

const paths: Record<AppIconName, React.ReactNode> = {
  brand: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="m6.5 16 4-4 3 3 3.5-5 3.5 6" />
      <circle cx="9" cy="9" r="1" />
    </>
  ),
  dashboard: (
    <>
      <rect x="3" y="3" width="7" height="7" rx="2" />
      <rect x="14" y="3" width="7" height="7" rx="2" />
      <rect x="3" y="14" width="7" height="7" rx="2" />
      <rect x="14" y="14" width="7" height="7" rx="2" />
    </>
  ),
  import: (
    <>
      <path d="M12 3v12" />
      <path d="m7.5 10.5 4.5 4.5 4.5-4.5" />
      <path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
    </>
  ),
  review: (
    <>
      <path d="M3 5.5A2.5 2.5 0 0 1 5.5 3h13A2.5 2.5 0 0 1 21 5.5v10a2.5 2.5 0 0 1-2.5 2.5h-8L6 21v-3h-.5A2.5 2.5 0 0 1 3 15.5z" />
      <path d="m8 10 2.5 2.5L16 7" />
    </>
  ),
  commit: (
    <>
      <path d="M4 8h16v12H4z" />
      <path d="M7 4h10l3 4H4z" />
      <path d="M9 13h6" />
    </>
  ),
  recovery: (
    <>
      <path d="M5 8V4m0 0h4M5 4l3 3" />
      <path d="M5.8 7.2A8 8 0 1 1 4 14" />
      <path d="M12 8v5l3 2" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19 13.5a7 7 0 0 0 0-3l2-1.5-2-3.4-2.4 1a7 7 0 0 0-2.6-1.5L13.6 2h-4l-.4 3.1a7 7 0 0 0-2.6 1.5l-2.4-1-2 3.4 2 1.5a7 7 0 0 0 0 3l-2 1.5 2 3.4 2.4-1A7 7 0 0 0 9.2 19l.4 3h4l.4-3a7 7 0 0 0 2.6-1.6l2.4 1 2-3.4z" />
    </>
  ),
  arrow: (
    <>
      <path d="M5 12h14" />
      <path d="m14 7 5 5-5 5" />
    </>
  ),
};

export function AppIcon({ name, size = 20, ...props }: AppIconProps) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      focusable="false"
      {...props}
    >
      {paths[name]}
    </svg>
  );
}
