import { useEffect, useMemo, useRef, type MouseEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import type { RenderedNote, ResourceData } from "../types";

const blobCache = new Map<string, string>();

function toBlobUrl(data: ResourceData): string {
  const binary = atob(data.base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return URL.createObjectURL(new Blob([bytes], { type: data.mime }));
}

interface Props {
  rendered: RenderedNote | null;
  onOpenNote: (id: string) => void;
}

export default function NotePreview({ rendered, onOpenNote }: Props) {
  const root = useRef<HTMLDivElement>(null);

  // Rust emits inert `joplin-resource://` URLs. Swap them for data attributes
  // before the HTML ever hits the DOM, then hydrate the bytes over IPC.
  const html = useMemo(
    () =>
      (rendered?.html ?? "").replace(
        /src="joplin-resource:\/\/([0-9a-fA-F]{32})"/g,
        (_match, id: string) => `data-resource="${id}"`,
      ),
    [rendered],
  );

  useEffect(() => {
    const container = root.current;
    if (!container) return;
    let alive = true;
    const images = Array.from(
      container.querySelectorAll<HTMLImageElement>("img[data-resource]"),
    );
    for (const img of images) {
      const id = img.dataset.resource;
      if (!id) continue;
      const cached = blobCache.get(id);
      if (cached) {
        img.src = cached;
        continue;
      }
      api
        .readResource(id)
        .then((data) => {
          if (!alive) return;
          const url = toBlobUrl(data);
          blobCache.set(id, url);
          img.src = url;
        })
        .catch(() => {
          if (alive) img.alt = "附件加载失败";
        });
    }
    return () => {
      alive = false;
    };
  }, [html]);

  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    const anchor = (event.target as HTMLElement).closest("a");
    if (!anchor) return;
    const href = anchor.getAttribute("href") ?? "";
    if (href.startsWith("joplin-note://")) {
      event.preventDefault();
      onOpenNote(href.slice("joplin-note://".length));
    } else if (href.startsWith("joplin-resource://")) {
      event.preventDefault();
    } else if (href.startsWith("#")) {
      event.preventDefault();
      root.current
        ?.querySelector(`[id="${href.slice(1)}"]`)
        ?.scrollIntoView({ behavior: "smooth", block: "start" });
    } else if (/^https?:\/\//.test(href)) {
      event.preventDefault();
      void openUrl(href);
    }
  };

  if (!rendered) {
    return <div className="preview-empty">选择一篇笔记开始阅读</div>;
  }

  return (
    <div className="preview-scroll" ref={root}>
      {rendered.outline.length > 3 && (
        <aside className="outline">
          <div className="outline-title">目录</div>
          {rendered.outline.map((item) => (
            <button
              key={item.anchor}
              className={`outline-item level-${item.level}`}
              onClick={() =>
                root.current
                  ?.querySelector(`[id="${item.anchor}"]`)
                  ?.scrollIntoView({ behavior: "smooth", block: "start" })
              }
            >
              {item.text}
            </button>
          ))}
        </aside>
      )}
      <article className="markdown-body" onClick={handleClick}>
        <div dangerouslySetInnerHTML={{ __html: html }} />
      </article>
    </div>
  );
}
