import { MenuPopover } from "../primitives/Menu";

export type PostMenuActionItem = {
  kind?: "action";
  label: string;
  action: () => void;
  disabled?: boolean;
  danger?: boolean;
};

export type PostMenuItem = PostMenuActionItem | { kind: "separator" };

export function PostMenuPopover({
  position,
  items,
  onClose,
  widthClassName = "w-36",
}: {
  position: { top: number; left?: number; right?: number };
  items: readonly PostMenuItem[];
  onClose: () => void;
  widthClassName?: string;
}) {
  return (
    <MenuPopover
      position={position}
      items={items.map((item, index) =>
        item.kind === "separator"
          ? { kind: "separator", id: `${index}:separator` }
          : { ...item, id: `${index}:${item.label}` },
      )}
      onClose={onClose}
      widthClassName={widthClassName}
    />
  );
}
