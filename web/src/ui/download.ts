// SPDX-License-Identifier: GPL-3.0-or-later
/** Saves bytes as a file through the browser's normal download. */
export function download(name: string, data: BlobPart, type = "application/octet-stream") {
  const url = URL.createObjectURL(new Blob([data], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  // Revoked later: some browsers start the download after click() returns.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** Readable size: 2.0 MB, 12 KB, 300 B. */
export function formatSize(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} KB`;
  return `${n} B`;
}

/** Joins a card directory and a name; the root is "/". */
export const joinPath = (dir: string, name: string) => (dir === "/" ? `/${name}` : `${dir}/${name}`);

export function parentPath(path: string): string {
  const i = path.lastIndexOf("/");
  return i <= 0 ? "/" : path.slice(0, i);
}
