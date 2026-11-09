import { useEffect, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { inertResources, loadResources } from "../resources";

export interface PrintJob {
  title: string;
  folder: string;
  updatedLabel: string;
  tags: string[];
  html: string;
}

interface Props {
  job: PrintJob;
  /** Called once the sheet is laid out and every attachment has decoded, i.e.
   *  the moment it is safe to hand the page over to WebView2's printer. */
  onReady: () => void;
}

/** The whole window is replaced by this sheet while a PDF is generated: the
 *  export prints the *current DOM*, so the sheet has to be the only thing left
 *  on the page. It lives outside `.app-shell` for exactly that reason. */
export default function PrintSheet({ job, onReady }: Props) {
  const body = useRef<HTMLDivElement>(null);
  // A leading `# heading` that just repeats the title would print twice.
  const html = useMemo(() => {
    const template = document.createElement("template");
    template.innerHTML = inertResources(job.html);
    const first = template.content.firstElementChild;
    const heading = first?.textContent?.trim();
    if (first && first.tagName === "H1" && heading === job.title.trim()) {
      first.remove();
    }
    return template.innerHTML;
  }, [job.html, job.title]);

  useEffect(() => {
    let alive = true;
    const settle = async () => {
      if (body.current) await loadResources(body.current);
      await document.fonts?.ready.catch(() => {});
      // Two frames: one for React's commit, one for the paint it triggers.
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      );
      if (alive) onReady();
    };
    void settle();
    return () => {
      alive = false;
    };
  }, [html, onReady]);

  return createPortal(
    <div className="print-sheet" role="document" aria-label="PDF 导出预览">
      <div className="print-paper">
        <header className="print-head">
          <h1>{job.title || "无标题笔记"}</h1>
          <div className="print-meta">
            <span className="print-folder">
              {/* A real glyph, not 📖: emoji are not embedded as images in the
                  PDF and would print as tofu boxes. */}
              <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
                <path
                  d="M12 7v14M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.7"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
              {job.folder || "未归入笔记本"}
            </span>
            <span className="print-meta-sep">·</span>
            <span>更新于 {job.updatedLabel}</span>
            {job.tags.length > 0 && (
              <>
                <span className="print-meta-sep">·</span>
                <span>{job.tags.map((t) => `#${t}`).join("  ")}</span>
              </>
            )}
          </div>
        </header>
        <article
          ref={body}
          className="markdown-body print-body"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      </div>
    </div>,
    document.body,
  );
}
