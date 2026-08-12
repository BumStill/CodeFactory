// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

interface ImagePreviewProps {
  src: string;
  alt: string;
  thumbnailClassName: string;
  caption?: string;
  onError?: () => void;
  title?: string;
}

export function ImagePreview({
  src,
  alt,
  thumbnailClassName,
  caption,
  onError,
  title,
}: ImagePreviewProps) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open]);

  return (
    <>
      <button
        type="button"
        className="group/image inline-block max-w-full rounded-lg text-left align-top focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/70"
        onClick={() => setOpen(true)}
        aria-label={`放大查看 ${alt}`}
        title={title ?? `放大查看 ${alt}`}
      >
        <img
          src={src}
          alt={alt}
          className={thumbnailClassName}
          loading="lazy"
          onError={onError}
        />
        {caption && <span className="mt-1 block text-caption text-gray-500">{caption}</span>}
      </button>

      {open && createPortal(
        <div
          role="dialog"
          aria-modal="true"
          aria-label="图片预览"
          className="fixed inset-0 z-[100] flex items-center justify-center bg-black/80 p-4"
          onClick={() => setOpen(false)}
        >
          <button
            type="button"
            aria-label="关闭图片预览"
            title="关闭"
            className="absolute right-4 top-4 rounded-full bg-black/50 p-2 text-white/80 transition-colors hover:bg-white/15 hover:text-white"
            onClick={(event) => {
              event.stopPropagation();
              setOpen(false);
            }}
          >
            <X size={20} />
          </button>
          <figure
            className="flex max-h-full max-w-full flex-col items-center gap-3"
            onClick={(event) => event.stopPropagation()}
          >
            <img
              src={src}
              alt={alt}
              className="max-h-[85vh] max-w-[92vw] rounded-xl border border-white/20 bg-surface-2 object-contain shadow-2xl"
            />
            <figcaption className="max-w-[92vw] truncate rounded-full bg-black/50 px-3 py-1 text-label text-white/80">
              {caption ?? alt}
            </figcaption>
          </figure>
        </div>,
        document.body,
      )}
    </>
  );
}
