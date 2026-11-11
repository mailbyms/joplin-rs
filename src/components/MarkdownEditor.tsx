import { useEffect, useRef } from "react";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { languages } from "@codemirror/language-data";
import { tags as t } from "@lezer/highlight";
import { basicSetup } from "codemirror";
import { createResourceFromFile, imageFilesFrom } from "../attachments";

interface Props {
  value: string;
  onChange: (next: string) => void;
  onError?: (message: string) => void;
}

/**
 * Source-mode colours, following Textus's `editor/modes/source-highlight.ts`.
 * Each token reads a --src-* variable declared in styles.css, so the palette
 * follows the application theme; the fallbacks match the deep-blue canvas.
 * Markup characters stay muted while headings, emphasis, links, quotes,
 * lists and fenced code each get a distinct colour.
 */
const v = (name: string, fallback: string) => `var(${name}, ${fallback})`;
const text = v("--text", "#1f2733");
const muted = v("--src-marker", "#96a1b0");
const accent = v("--src-heading", "#1d4ed8");

export const sourceHighlightStyle = HighlightStyle.define([
  {
    tag: [
      t.processingInstruction,
      t.contentSeparator,
      t.meta,
      t.punctuation,
      t.bracket,
    ],
    color: muted,
  },
  { tag: t.escape, color: accent },
  {
    tag: [
      t.heading,
      t.heading1,
      t.heading2,
      t.heading3,
      t.heading4,
      t.heading5,
      t.heading6,
    ],
    color: accent,
    fontWeight: "600",
  },
  { tag: t.strong, color: v("--src-strong", "#16368f"), fontWeight: "600" },
  {
    tag: t.emphasis,
    color: v("--src-emphasis", "#2f6fbf"),
    fontStyle: "italic",
  },
  { tag: t.strikethrough, textDecoration: "line-through", opacity: "0.75" },
  {
    tag: t.quote,
    color: v("--src-quote", "#67707d"),
    fontStyle: "italic",
  },
  { tag: t.list, color: v("--src-list", "#4c8b5a") },
  {
    tag: t.link,
    color: v("--src-link", "#1c6fab"),
    textDecoration: "underline",
    textUnderlineOffset: "2px",
  },
  { tag: [t.url, t.labelName], color: v("--src-url", "#7a5aa6") },
  { tag: t.monospace, color: v("--src-code", "#16368f") },
  { tag: t.comment, color: muted, fontStyle: "italic" },
  { tag: t.string, color: v("--src-token-string", "#4c8b5a") },
  {
    tag: [t.number, t.bool, t.null],
    color: v("--src-token-number", "#a8761c"),
  },
  {
    tag: [t.keyword, t.operator, t.modifier],
    color: v("--src-token-keyword", "#16368f"),
  },
  {
    tag: [
      t.function(t.variableName),
      t.definition(t.variableName),
      t.className,
      t.typeName,
    ],
    color: v("--src-token-function", "#1c6fab"),
  },
  {
    tag: [t.tagName, t.attributeName],
    color: v("--src-token-tag", "#9a4a73"),
  },
  { tag: t.content, color: text },
]);

/** GFM Markdown grammar (plus lazily loaded languages for fenced code) and
 *  the palette above. basicSetup installs CodeMirror's default highlighter as
 *  a fallback; this one takes precedence without replacing basicSetup. */
function sourceHighlighting(): Extension {
  return [
    markdown({ base: markdownLanguage, codeLanguages: languages }),
    syntaxHighlighting(sourceHighlightStyle),
  ];
}

const editorTheme = EditorView.theme(
  {
    "&": {
      height: "100%",
      fontSize: "14px",
      backgroundColor: "transparent",
      color: "var(--text)",
    },
    ".cm-scroller": {
      fontFamily:
        '"Cascadia Code", "JetBrains Mono", "SFMono-Regular", Consolas, "Microsoft YaHei", monospace',
      lineHeight: "1.8",
      overflow: "auto",
    },
    ".cm-content": {
      padding: "14px 26px 160px",
      caretColor: "var(--accent)",
      minHeight: "100%",
    },
    ".cm-gutters": {
      backgroundColor: "transparent",
      border: "none",
      color: "var(--text-faint)",
      paddingRight: "6px",
    },
    ".cm-activeLine": { backgroundColor: "rgba(29, 78, 216, 0.05)" },
    ".cm-activeLineGutter": {
      backgroundColor: "transparent",
      color: "var(--accent)",
    },
    ".cm-selectionBackground": {
      backgroundColor: "rgba(29, 78, 216, 0.18) !important",
    },
    ".cm-cursor": {
      borderLeftColor: "var(--accent)",
      borderLeftWidth: "2px",
    },
    "&.cm-focused": { outline: "none" },
  },
);

/**
 * The parent gives this component a `key={note.id}` so a note switch simply
 * remounts the editor. No fragile bidirectional document synchronization.
 */
export default function MarkdownEditor({ value, onChange, onError }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  /** Register each dropped/pasted image as a Joplin resource and insert
   *  `![](:/<id>)` at the caret. CodeMirror's basicSetup has no image handler
   *  of its own, so without this the paste is silently ignored. */
  const insertFiles = (view: EditorView, files: File[], extraText: string) => {
    void (async () => {
      const tags: string[] = [];
      for (const file of files) {
        try {
          tags.push(`![](${await createResourceFromFile(file)})`);
        } catch (e) {
          onErrorRef.current?.(`插入图片失败：${String(e)}`);
        }
      }
      if (tags.length === 0) return;
      const text = `${extraText ? `${extraText}\n` : ""}${tags.join("\n")}\n`;
      const at = view.state.selection.main.from;
      view.dispatch({
        changes: { from: at, insert: text },
        selection: { anchor: at + text.length },
      });
      view.focus();
    })();
  };

  useEffect(() => {
    if (!host.current) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          sourceHighlighting(),
          EditorView.lineWrapping,
          editorTheme,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              onChangeRef.current(update.state.doc.toString());
            }
          }),
          EditorView.domEventHandlers({
            paste(event, editor) {
              const data = (event as ClipboardEvent).clipboardData;
              const files = imageFilesFrom(data);
              if (files.length === 0) return false;
              event.preventDefault();
              insertFiles(editor, files, data?.getData("text/plain") ?? "");
              return true;
            },
            drop(event, editor) {
              const data = (event as DragEvent).dataTransfer;
              const files = imageFilesFrom(data);
              if (files.length === 0) return false;
              event.preventDefault();
              insertFiles(editor, files, "");
              return true;
            },
          }),
        ],
      }),
      parent: host.current,
    });
    return () => view.destroy();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return <div className="cm-host" ref={host} />;
}
