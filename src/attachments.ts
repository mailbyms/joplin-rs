import { api } from "./api";

/**
 * Joplin attachments are resources, not inline bytes. Milkdown's default
 * uploader turns a picked/pasted image into a `data:` URL, which lives only in
 * the note body and is invisible to the WebDAV synchroniser. Everything in
 * this module implements Joplin's `createResourceFromPath()` equivalent:
 * register the bytes through Rust, then store `:/<32-hex-id>` in the body.
 */

const IMAGE_EXT = /\.(png|jpe?g|gif|webp|bmp|svg|avif|heic)$/i;
const JOPLIN_REF = /^:\/([0-9a-fA-F]{32})$/;

/** Files from a paste or a drop. Chromium leaves `files` empty on paste, so
 *  the `items` list has to be inspected as well. */
export function imageFilesFrom(data: DataTransfer | null): File[] {
  if (!data) return [];
  const found = new Map<string, File>();
  const key = (f: File) => `${f.name}:${f.size}:${f.lastModified}`;
  for (const file of Array.from(data.files ?? [])) {
    if (file.type.startsWith("image/") || IMAGE_EXT.test(file.name)) {
      found.set(key(file), file);
    }
  }
  for (const item of Array.from(data.items ?? [])) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (!file) continue;
    if (!item.type.startsWith("image/") && !IMAGE_EXT.test(file.name)) continue;
    found.set(key(file), file);
  }
  return [...found.values()];
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result ?? "");
      const comma = url.indexOf(",");
      resolve(comma >= 0 ? url.slice(comma + 1) : url);
    };
    reader.onerror = () => reject(reader.error ?? new Error("读取图片失败"));
    reader.readAsDataURL(file);
  });
}

/** Register one file and return the `:/<id>` token for the note body. */
export async function createResourceFromFile(file: File): Promise<string> {
  const base64 = await fileToBase64(file);
  const resource = await api.createResource({
    base64,
    mime: file.type || "",
    filename: file.name || "",
  });
  return `:/${resource.id}`;
}

/** Milkdown's `proxyDomURL` hook: keep `:/id` in the document but give the
 *  DOM an actual image source. Non-resource URLs pass through untouched. */
export async function resourceDataUrl(url: string): Promise<string> {
  const match = JOPLIN_REF.exec(url);
  if (!match) return url;
  try {
    const resource = await api.readResource(match[1]);
    return `data:${resource.mime || "application/octet-stream"};base64,${resource.base64}`;
  } catch {
    // A resource whose blob has not been downloaded yet simply stays broken.
    return url;
  }
}

const INLINE_IMAGE =
  /!\[([^\]]*)\]\(\s*data:(image\/[a-zA-Z0-9.+-]+);base64,([A-Za-z0-9+/=\s]+?)\s*\)/g;

/**
 * Safety net for notes that already contain an inline `data:` image (pasted
 * before the resource pipeline existed, or typed in by hand). Rewrites each
 * one into a real resource so the next sync uploads its blob. Any failed
 * conversion leaves the original markup intact — never drop note content.
 */
export async function normalizeInlineImages(body: string): Promise<string> {
  if (!body.includes("data:image")) return body;
  const matches = Array.from(body.matchAll(INLINE_IMAGE));
  if (matches.length === 0) return body;
  let out = "";
  let cursor = 0;
  for (const match of matches) {
    const [whole, alt, mime, data] = match;
    const start = match.index ?? 0;
    out += body.slice(cursor, start);
    try {
      const resource = await api.createResource({
        base64: data.replace(/\s/g, ""),
        mime,
        filename: "",
      });
      out += `![${alt}](:/${resource.id})`;
    } catch {
      out += whole;
    }
    cursor = start + whole.length;
  }
  return out + body.slice(cursor);
}
