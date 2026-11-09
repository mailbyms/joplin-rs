import { api } from "./api";
import type { ResourceData } from "./types";

/** Joplin resources are emitted as inert `joplin-resource://<id>` URLs; the
 *  bytes have to be pulled over IPC and turned into blob URLs. */
const cache = new Map<string, string>();

const RESOURCE_SRC = /src="joplin-resource:\/\/([0-9a-fA-F]{32})"/g;

function toBlobUrl(data: ResourceData): string {
  const binary = atob(data.base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return URL.createObjectURL(new Blob([bytes], { type: data.mime }));
}

/** Swap resource URLs for data attributes so no bogus request is attempted. */
export function inertResources(html: string): string {
  return html.replace(
    RESOURCE_SRC,
    (_match, id: string) => `data-resource="${id}"`,
  );
}

/** Resolve every image inside `root`. Never rejects: a broken attachment should
 *  not block a PDF export or a preview. */
export async function loadResources(root: HTMLElement): Promise<void> {
  const images = Array.from(
    root.querySelectorAll<HTMLImageElement>("img[data-resource]"),
  );
  await Promise.all(
    images.map(async (img) => {
      const id = img.dataset.resource;
      if (!id) return;
      try {
        let url = cache.get(id);
        if (!url) {
          url = toBlobUrl(await api.readResource(id));
          cache.set(id, url);
        }
        img.src = url;
        await img.decode?.().catch(() => {});
      } catch {
        img.alt = "附件加载失败";
      }
    }),
  );
}
