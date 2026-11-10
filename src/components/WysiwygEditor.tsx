import { useEffect, useRef, useState } from "react";
import { Crepe } from "@milkdown/crepe";
// classic.css only declares the --crepe-* palette. Component styles live in
// separate files: they position the toolbar, slash menu, link tooltip and
// block handle, hide inactive widgets, and drop the contenteditable focus
// ring. Without them every widget is a bare block stacked in document order.
// Import only the enabled features' sheets: latex.css would pull the whole
// KaTeX font set (~1 MB) into the bundle for a feature we explicitly disable.
import "@milkdown/crepe/theme/common/prosemirror.css";
import "@milkdown/crepe/theme/common/reset.css";
import "@milkdown/crepe/theme/common/block-edit.css";
import "@milkdown/crepe/theme/common/code-mirror.css";
import "@milkdown/crepe/theme/common/cursor.css";
import "@milkdown/crepe/theme/common/image-block.css";
import "@milkdown/crepe/theme/common/link-tooltip.css";
import "@milkdown/crepe/theme/common/list-item.css";
import "@milkdown/crepe/theme/common/placeholder.css";
import "@milkdown/crepe/theme/common/toolbar.css";
import "@milkdown/crepe/theme/common/table.css";
import "@milkdown/crepe/theme/common/top-bar.css";
import "@milkdown/crepe/theme/classic.css";
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";
import { api } from "../api";

/** Fenced code blocks are CodeMirror instances, and Crepe ships them unthemed:
 *  the tokens fall back to a dark-editor palette that is unreadable on the
 *  light page the notes are rendered on. This is GitHub's code palette, which
 *  is what the rest of the typography follows (see `github.css`). */
const codeSurfaceTheme = EditorView.theme(
  {
    "&": { color: "#24292e", backgroundColor: "transparent" },
    ".cm-content": { caretColor: "#044289" },
    ".cm-gutters": { backgroundColor: "transparent", color: "#b3bac5", border: "none" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    ".cm-activeLineGutter": { backgroundColor: "transparent" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "#cbe3ff",
    },
  },
  { dark: false },
);

const codeSyntax = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.operatorKeyword, tags.controlKeyword], color: "#d73a49" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], color: "#032f62" },
  { tag: [tags.comment, tags.lineComment, tags.blockComment], color: "#6a737d", fontStyle: "italic" },
  { tag: [tags.number, tags.bool, tags.null, tags.atom], color: "#005cc5" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: "#6f42c1" },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.attributeName], color: "#6f42c1" },
  { tag: [tags.tagName], color: "#22863a" },
  { tag: [tags.propertyName, tags.variableName], color: "#24292e" },
  { tag: [tags.heading], color: "#005cc5", fontWeight: "bold" },
  { tag: [tags.link, tags.url], color: "#032f62" },
  { tag: [tags.emphasis], fontStyle: "italic" },
  { tag: [tags.strong], fontWeight: "bold" },
  { tag: [tags.meta, tags.punctuation, tags.operator, tags.bracket], color: "#24292e" },
]);

interface Props {
  value: string;
  onChange: (next: string) => void;
  onOpenNote: (id: string) => void;
  onFallback: () => void;
}

const JOPLIN_LINK = /^:\/([0-9a-fA-F]{32})$/;

function withoutCode(src: string): string {
  return src
    .replace(/```[\s\S]*?```/g, "")
    .replace(/~~~[\s\S]*?~~~/g, "")
    .replace(/`[^`\n]*`/g, "");
}

function joplinIds(src: string): string {
  return (src.match(/:\/[0-9a-fA-F]{32}/g) ?? []).sort().join(",");
}

/**
 * Milkdown parses Markdown with remark and serialises it back, which rewrites
 * formatting. Cosmetic rewriting is fine, but silently dropping content is not:
 * the result would be saved to SQLite and then propagated to every other device
 * over WebDAV. Refuse to write back whenever a round-trip loses anything.
 */
export function unsafeRoundTrip(src: string, round: string): string | null {
  if (/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/.test(src)) {
    return "YAML front matter";
  }
  if (/<[a-zA-Z!/][^>]*>/.test(withoutCode(src))) {
    return "原始 HTML";
  }
  if (joplinIds(src) !== joplinIds(round)) {
    return "Joplin 附件或内部笔记链接";
  }
  // remark escaping and list normalisation usually make the text slightly
  // LONGER, which is harmless. Only a substantial shrink means nodes were
  // actually dropped, so a symmetric length delta produced too many false
  // positives on notes with tables or long link URLs.
  const loss = src.length - round.length;
  if (loss > Math.max(200, src.length * 0.25)) {
    return "部分内容";
  }
  return null;
}

/**
 * WYSIWYG editing via Milkdown/ProseMirror. The UI labels this view "打字机",
 * but the behaviour is plain Typora-style live preview — no extra scrolling
 * or caret logic.
 * The Markdown string is the single source of truth: this component is mounted
 * with `key={note.id}`, so switching notes or modes simply destroys and
 * recreates the editor. There is no caret/document synchronisation with the
 * CodeMirror source editor.
 */
export default function WysiwygEditor({
  value,
  onChange,
  onOpenNote,
  onFallback,
}: Props) {
  const host = useRef<HTMLDivElement>(null);
  const onChangeRef = useRef(onChange);
  const onOpenNoteRef = useRef(onOpenNote);
  onChangeRef.current = onChange;
  onOpenNoteRef.current = onOpenNote;
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    let cancelled = false;
    let ready = false;
    let destroyed = false;
    let lastMarkdown = value;
    let observer: MutationObserver | null = null;
    const blobCache = new Map<string, string>();

    const crepe = new Crepe({
      root: el,
      defaultValue: value,
      features: {
        [Crepe.Feature.AI]: false,
        [Crepe.Feature.Latex]: false,
      },
      featureConfigs: {
        [Crepe.Feature.CodeMirror]: {
          theme: [codeSurfaceTheme, syntaxHighlighting(codeSyntax)],
        },
      },
    });

    // `on` must be called before `create()`. The first markdownUpdated event
    // fires while the initial document is parsed; `ready` stays false until the
    // round-trip check has passed, so switching modes never rewrites a note.
    crepe.on((listener) => {
      listener.markdownUpdated((_ctx, markdown) => {
        if (!ready || cancelled || markdown === lastMarkdown) return;
        lastMarkdown = markdown;
        onChangeRef.current(markdown);
      });
    });

    // Joplin stores attachments as `:/<32 hex>`. Keep that token in the
    // ProseMirror document (so serialisation round-trips) and only swap the DOM
    // image source for base64 bytes fetched over IPC.
    const patchJoplinLinks = () => {
      el.querySelectorAll<HTMLImageElement>('img[src^=":/"]').forEach((img) => {
        const match = JOPLIN_LINK.exec(img.getAttribute("src") ?? "");
        if (!match) return;
        const id = match[1];
        const cached = blobCache.get(id);
        if (cached) {
          img.src = cached;
          return;
        }
        if (img.dataset.pending === id) return;
        img.dataset.pending = id;
        void api
          .readResource(id)
          .then((resource) => {
            if (cancelled) return;
            const url = `data:${resource.mime};base64,${resource.base64}`;
            blobCache.set(id, url);
            img.src = url;
          })
          .catch(() => {
            /* a missing blob simply stays as an unloaded image */
          });
      });
    };

    // Internal note links use the same `:/id` form; open them inside the app.
    const onClick = (event: MouseEvent) => {
      const anchor = (event.target as HTMLElement | null)?.closest("a");
      const match = JOPLIN_LINK.exec(anchor?.getAttribute("href") ?? "");
      if (!match) return;
      event.preventDefault();
      event.stopPropagation();
      onOpenNoteRef.current(match[1]);
    };

    const afterCreate = () => {
      const round = crepe.getMarkdown();
      const reason = unsafeRoundTrip(value, round);
      if (reason) {
        setNotice(
          `此笔记含${reason}，打字机视图无法无损还原，已设为只读以保护原文。请用「源码」模式编辑。`,
        );
        crepe.setReadonly(true);
      } else {
        if (round.replace(/\r\n/g, "\n").trim() !== value.replace(/\r\n/g, "\n").trim()) {
          setNotice(
            "该视图会规范化 Markdown 排版（列表符号、空行等），内容不会丢失。",
          );
        }
        ready = true;
      }
      observer = new MutationObserver(patchJoplinLinks);
      observer.observe(el, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["src"],
      });
      patchJoplinLinks();
    };

    el.addEventListener("click", onClick, true);
    void crepe
      .create()
      .then(() => {
        if (cancelled) {
          destroyed = true;
          void crepe.destroy();
          return;
        }
        try {
          afterCreate();
        } catch (e) {
          setNotice(`编辑器初始化异常：${String(e)}`);
          crepe.setReadonly(true);
        }
      })
      .catch((e) => setNotice(`编辑器加载失败：${String(e)}`));

    return () => {
      cancelled = true;
      ready = false;
      observer?.disconnect();
      el.removeEventListener("click", onClick, true);
      if (!destroyed) {
        destroyed = true;
        void crepe.destroy();
      }
    };
    // Remounted by key when the note changes, exactly like MarkdownEditor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="wysiwyg-wrap">
      {notice && (
        <div className="wysiwyg-notice">
          <span>{notice}</span>
          <button type="button" onClick={onFallback}>
            切回源码
          </button>
        </div>
      )}
      <div className="wysiwyg-host" ref={host} />
    </div>
  );
}
