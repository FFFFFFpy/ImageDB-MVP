import type { ReactNode } from 'react';
import { Button, type ButtonProps } from './Button';

export interface IconButtonProps extends Omit<ButtonProps, 'children' | 'aria-label'> {
  label: string;
  icon: ReactNode;
}

export function IconButton({ label, icon, className, ...props }: IconButtonProps) {
  return (
    <Button
      {...props}
      aria-label={label}
      title={props.title ?? label}
      className={['imagedb-icon-button', className].filter(Boolean).join(' ')}
    >
      {icon}
    </Button>
  );
}
