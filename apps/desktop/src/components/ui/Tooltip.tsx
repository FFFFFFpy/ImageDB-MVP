import {
  Tooltip as AnimalTooltip,
  type TooltipProps as AnimalTooltipProps,
} from 'animal-island-ui';

export interface TooltipProps extends Omit<AnimalTooltipProps, 'title' | 'variant'> {
  content: AnimalTooltipProps['title'];
}

export function Tooltip({ content, bordered = true, children, ...props }: TooltipProps) {
  return (
    <AnimalTooltip {...props} title={content} variant="default" bordered={bordered}>
      {children}
    </AnimalTooltip>
  );
}
