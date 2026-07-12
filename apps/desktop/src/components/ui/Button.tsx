import { Button as AnimalButton, type ButtonProps as AnimalButtonProps } from 'animal-island-ui';
import classNames from 'classnames';

export type ButtonVariant = 'primary' | 'secondary' | 'quiet' | 'danger';

export interface ButtonProps extends Omit<
  AnimalButtonProps,
  'type' | 'danger' | 'loading' | 'htmlType'
> {
  variant?: ButtonVariant;
  loading?: boolean;
  loadingLabel?: string;
  nativeType?: 'button' | 'submit' | 'reset';
}

const variantProps: Record<ButtonVariant, Pick<AnimalButtonProps, 'type' | 'danger'>> = {
  primary: { type: 'primary' },
  secondary: { type: 'default' },
  quiet: { type: 'text' },
  danger: { type: 'primary', danger: true },
};

export function Button({
  variant = 'secondary',
  loading = false,
  loadingLabel = '处理中…',
  nativeType = 'button',
  className,
  children,
  disabled,
  ...props
}: ButtonProps) {
  const animalProps = variantProps[variant];

  return (
    <AnimalButton
      {...props}
      {...animalProps}
      className={classNames('imagedb-button', `imagedb-button--${variant}`, className)}
      htmlType={nativeType}
      loading={loading}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
    >
      {loading ? loadingLabel : children}
    </AnimalButton>
  );
}
