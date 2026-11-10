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
    // `!important` is load-bearing. CodeMirror's own base theme ships
    // `&dark.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground
    // { background: #233 }`, and that four-class selector outranks anything a
    // normal theme rule can produce — the view reports `dark` because Milkdown
    // creates it without a light theme, so selecting text in a code block used
    // to paint near-black behind near-black tokens. The source editor has the
    // same override for the same reason (see MarkdownEditor.tsx).
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "#cbe3ff !important",
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

/**
 * Blanks fenced blocks and inline code **in place** — every character becomes a
 * space but line breaks stay — so a match offset in the result still maps back
 * to the line the user sees in 源码 mode. (`unsafeRoundTrip` used to strip code
 * out entirely, which was fine for a yes/no answer but lost the position.)
 */
function maskCode(src: string): string {
  const blank = (m: string) => m.replace(/[^\n]/g, " ");
  return src
    .replace(/```[\s\S]*?```/g, blank)
    .replace(/~~~[\s\S]*?~~~/g, blank)
    .replace(/`[^`\n]*`/g, blank);
}

/** 1-based line number of `index` inside `text`. */
function lineOf(text: string, index: number): number {
  return text.slice(0, index).split("\n").length;
}

function joplinIds(src: string): string {
  return (src.match(/:\/[0-9a-fA-F]{32}/g) ?? []).sort().join(",");
}

export interface RoundTripIssue {
  /** What the 打字机 view cannot preserve, e.g. "原始 HTML". */
  reason: string;
  /** 1-based source line when the culprit is locatable, 0 otherwise. */
  line: number;
}

/**
 * What counts as raw HTML: `<!--`, `<!DOCTYPE`, or a real tag — `<` + name +
 * (space | `/` | `>`). The shape matters: `<https://…>` and `<user@host>` are
 * Markdown autolinks, which remark parses to a link and serialises back
 * unchanged, so treating them as HTML locked dozens of perfectly safe notes
 * into read-only. A tag name is followed by `:` or `@` in those, never by
 * whitespace, a slash or `>`.
 *
 * The `(?<!\\)` skips escaped brackets (`\<VirtualHost …\>`), which are literal
 * text in Markdown — Apache/nginx config snippets in notes are usually written
 * that way and are not HTML at all.
 */
const HTML_TAG = /(?<!\\)(?:<!--|<![A-Za-z]|<\/?[A-Za-z][A-Za-z0-9-]*(?=[\s/>]))/;

/**
 * Milkdown parses Markdown with remark and serialises it back, which rewrites
 * formatting. Cosmetic rewriting is fine, but silently dropping content is not:
 * the result would be saved to SQLite and then propagated to every other device
 * over WebDAV. Refuse to write back whenever a round-trip loses anything.
 */
export function unsafeRoundTrip(src: string, round: string): RoundTripIssue | null {
  if (/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/.test(src)) {
    return { reason: "YAML front matter", line: 1 };
  }
  const masked = maskCode(src);
  const html = HTML_TAG.exec(masked);
  if (html) {
    return { reason: "原始 HTML", line: lineOf(masked, html.index) };
  }
  if (joplinIds(src) !== joplinIds(round)) {
    return { reason: "Joplin 附件或内部笔记链接", line: 0 };
  }
  // remark escaping and list normalisation usually make the text slightly
  // LONGER, which is harmless. Only a substantial shrink means nodes were
  // actually dropped, so a symmetric length delta produced too many false
  // positives on notes with tables or long link URLs.
  const loss = src.length - round.length;
  if (loss > Math.max(200, src.length * 0.25)) {
    return { reason: "部分内容", line: 0 };
  }
  return null;
}

/** The banner text. A locatable issue names the line so 源码 mode can jump to it. */
export function readonlyNotice(issue: RoundTripIssue): string {
  return issue.line > 0
    ? `此笔记第 ${issue.line} 行含${issue.reason}，已设为只读以保护原文。请用「源码」模式编辑。`
    : `此笔记含${issue.reason}，打字机视图无法无损还原，已设为只读以保护原文。请用「源码」模式编辑。`;
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
        // The block menu is what the "+" handle and a leading "/" open. Crepe
        // hard-codes its labels in English; only the label changes here — the
        // icons, the group order and the commands stay Crepe's own. Math is
        // absent because the Latex feature is off above.
        [Crepe.Feature.BlockEdit]: {
          textGroup: {
            label: "文本",
            text: { label: "正文" },
            h1: { label: "标题 1" },
            h2: { label: "标题 2" },
            h3: { label: "标题 3" },
            h4: { label: "标题 4" },
            h5: { label: "标题 5" },
            h6: { label: "标题 6" },
            quote: { label: "引用" },
            divider: { label: "分割线" },
          },
          listGroup: {
            label: "列表",
            bulletList: { label: "无序列表" },
            orderedList: { label: "有序列表" },
            taskList: { label: "任务列表" },
          },
          advancedGroup: {
            label: "插入",
            image: { label: "图片" },
            codeBlock: { label: "代码块" },
            table: { label: "表格" },
          },
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
      const issue = unsafeRoundTrip(value, round);
      if (issue) {
        setNotice(readonlyNotice(issue));
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
