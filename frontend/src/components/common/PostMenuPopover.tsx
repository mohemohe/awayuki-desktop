import { MenuPopover } from "../primitives/Menu";

export function PostMenuPopover({
  position,
  items,
  onClose,
  widthClassName = "w-36",
}: {
  position: { top: number; left?: number; right?: number };
  items: Array<{
    label: string;
    action: () => void;
    disabled?: boolean;
    danger?: boolean;
  }>;
  onClose: () => void;
  widthClassName?: string;
}) {
  return (
    <MenuPopover
      position={position}
      items={items.map((item, index) => ({
        ...item,
        id: `${index}:${item.label}`,
      }))}
      onClose={onClose}
      widthClassName={widthClassName}
    />
  );
}
