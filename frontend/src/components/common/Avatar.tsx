import { useAppStore } from "../../store/appStore";
import type { AppearanceSettings } from "../../types/app";
import { avatarShapeClass } from "../../utils/format";
import { uniqueMediaSources } from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";

type AvatarSize = "xs" | "md" | "lg" | "post" | "xl" | "xxl";

const AVATAR_SIZE_CLASS: Record<AvatarSize, string> = {
  xs: "h-4 w-4",
  md: "h-6 w-6",
  lg: "h-8 w-8",
  post: "h-9 w-9",
  xl: "h-12 w-12",
  xxl: "h-16 w-16",
};

export function Avatar({
  src,
  sources,
  label,
  size = "md",
  shape,
}: {
  src?: string | null;
  sources?: Array<string | null | undefined>;
  label: string;
  size?: AvatarSize;
  shape?: AppearanceSettings["avatar_shape"];
}) {
  const configuredShape = useAppStore(
    (state) => state.snapshot?.settings.appearance.avatar_shape ?? "Rounded",
  );
  const mediaSources = uniqueMediaSources(sources ?? [src]);
  const image = useRetriedMediaSource(mediaSources);
  const className = AVATAR_SIZE_CLASS[size];
  const shapeClass = avatarShapeClass(shape ?? configuredShape);

  return (
    <div className={`${className} ${shapeClass} relative shrink-0 bg-surface1`}>
      {image.src && !image.failed ? (
        <img
          key={image.key}
          src={image.src}
          alt={label}
          className={`absolute inset-0 h-full w-full object-cover ${shapeClass} ${image.loaded ? "" : "opacity-0"}`}
          onLoad={image.onLoad}
          onError={image.onError}
        />
      ) : null}
      {!image.loaded ? (
        <div
          className={`absolute inset-0 grid place-items-center ${shapeClass} text-xs font-bold text-text`}
          aria-hidden={Boolean(image.src && !image.failed)}
        >
          {label.slice(0, 1)}
        </div>
      ) : null}
    </div>
  );
}
