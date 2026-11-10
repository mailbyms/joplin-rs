import { useEffect, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { inertResources, loadResources } from "../resources";
import type { PdfHeading } from "../types";

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
   *  the moment it is safe to hand the page over to WebView2's printer. The
   *  headings travel with it so the PDF can be given a bookmark tree. */
  onReady: (headings: PdfHeading[]) => void;
}

/** Anchor of the note title itself, which is rendered outside the note body. */
const TITLE_ANCHOR = "jr-h-title";

/** The whole window is replaced by this sheet while a PDF is generated: the
 *  export prints the *current DOM*, so the sheet has to be the only thing left
 *  on the page. It lives outside `.app-shell` for exactly that reason. */
export default function PrintSheet({ job, onReady }: Props) {
  const body = useRef<HTMLDivElement>(null);

  const { html, headings } = useMemo(() => {
    const template = document.createElement("template");
    template.innerHTML = inertResources(job.html);
    // A leading `# heading` that just repeats the title would print twice.
    const first = template.content.firstElementChild;
    const heading = first?.textContent?.trim();
    if (first && first.tagName === "H1" && heading === job.title.trim()) {
      first.remove();
    }

    // WebView2 only records a destination for anchors something actually links
    // to, so each heading gets an id *and* a self-link wrapped around its
    // content. The link is invisible: `.print-anchor` inherits colour and drops
    // the underline. Rust then reads those destinations back out of the PDF
    // (see `outline.rs`) to build the bookmark tree.
    const found: PdfHeading[] = [];
    template.content.querySelectorAll("h1,h2,h3,h4,h5,h6").forEach((el, index) => {
      const text = el.textContent?.trim() ?? "";
      if (!text) return;
      const anchor = `jr-h-${index}`;
      el.id = anchor;
      // A heading that already contains a link cannot take another one (links
      // do not nest), so it keeps the id and simply has no bookmark.
      if (!el.querySelector("a")) {
        el.innerHTML = `<a class="print-anchor" href="#${anchor}">${el.innerHTML}</a>`;
      }
      found.push({ level: Number(el.tagName.slice(1)), title: text, anchor });
    });

    const title = job.title.trim();
    return {
      html: template.innerHTML,
      headings: title
        ? [{ level: 1, title, anchor: TITLE_ANCHOR }, ...found]
        : found,
    };
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
      if (alive) onReady(headings);
    };
    void settle();
    return () => {
      alive = false;
    };
  }, [html, headings, onReady]);

  // WebView2 takes the PDF's /Title from the document title; without this every
  // export would be filed as "Joplin RS". Restored on unmount.
  useEffect(() => {
    const previous = document.title;
    document.title = job.title.trim() || "无标题笔记";
    return () => {
      document.title = previous;
    };
  }, [job.title]);

  return createPortal(
    <div className="print-sheet" role="document" aria-label="PDF 导出预览">
      <div className="print-paper">
        <header className="print-head">
          <h1 id={TITLE_ANCHOR}>
            <a className="print-anchor" href={`#${TITLE_ANCHOR}`}>
              {job.title || "无标题笔记"}
            </a>
          </h1>
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
