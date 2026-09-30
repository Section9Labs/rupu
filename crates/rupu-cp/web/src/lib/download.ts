// Browser download hand-off shared by the finding-report export surfaces (the
// project report dialog and the per-finding buttons).

/** How long the object URL stays valid after the click. Revoking any sooner
 *  can drop a large download in Firefox and Safari, which read the URL
 *  asynchronously after the click. */
export const REVOKE_DELAY_MS = 30_000;

/** Offer `blob` to the browser as a file download. The file is named after
 *  `blob` itself when it is a named `File` (the server's `Content-Disposition`
 *  filename, see `api.exportFindings`), else `fallbackName`. */
export function saveBlob(blob: Blob, fallbackName: string): void {
  const name = blob instanceof File && blob.name ? blob.name : fallbackName;
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.hidden = true;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), REVOKE_DELAY_MS);
}
