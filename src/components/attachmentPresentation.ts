// SPDX-License-Identifier: Apache-2.0
export function formatAttachmentSize(bytes: number): string {
  return bytes >= 1024 * 1024 ? `${(bytes / (1024 * 1024)).toFixed(1)} MB` : `${Math.max(0.1, bytes / 1024).toFixed(1)} KB`;
}
export function attachmentType(name: string): string {
  return name.split(".").pop()?.toUpperCase() || "FILE";
}
/** Encode user-controlled labels and destinations without changing vision image syntax. */
export function attachmentLink(name: string, path: string, image = false): string {
  const label = name.replace(/[\\\[\]]/g, "\\$&").replace(/[\r\n]/g, " ");
  const url = (path.startsWith("file://") ? path : `file://${path}`).replace(/[<>\r\n()]/g, (v) => encodeURIComponent(v));
  return `${image ? "!" : ""}[${label}](${/\s/.test(url) ? `<${url}>` : url})`;
}
