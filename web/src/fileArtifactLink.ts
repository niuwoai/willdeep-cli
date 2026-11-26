const ARTIFACT_EXTENSIONS = new Set([
  "md", "markdown", "js", "mjs", "cjs", "ts", "tsx", "jsx", "json", "py", "rs", "go", "css", "scss", "html", "htm",
  "yaml", "yml", "sh", "bash", "zsh", "sql", "toml", "xml", "svg", "vue", "svelte", "txt", "csv", "log", "diff",
]);

export type FileArtifactRef = { name: string; path: string; ext: string };

export function fileArtifactFromHref(href: string): FileArtifactRef | null {
  const target = href.split("#", 1)[0];
  const local = target.replace(/^file:\/\/(?=\/)/i, "").replace(/^\/(?=[a-z]:\/)/i, "");
  if (/^[a-z][a-z\d+.-]*:/i.test(local) && !/^[a-z]:\//i.test(local)) return null;
  if (local.includes("?")) return null;

  let path: string;
  try {
    path = decodeURIComponent(local);
  } catch {
    return null;
  }
  if (path.startsWith("//") || /[\\\p{Cc}]/u.test(path) || path.split("/").includes("..")) return null;
  const name = path.split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  const ext = name.slice(dot + 1).toLowerCase();
  return dot > 0 && ARTIFACT_EXTENSIONS.has(ext) ? { name, path, ext } : null;
}

export function isBrowserLink(href: string): boolean {
  if (!/^https?:\/\//i.test(href)) return false;
  try {
    const url = new URL(href);
    // Serialization inserts any credentials before the origin's host.
    return url.href.startsWith(`${url.origin}/`);
  } catch {
    return false;
  }
}

export function fileArtifactGlyph(ext: string): string {
  switch (ext) {
    case "md":
    case "markdown":
      return "MD";
    case "js":
    case "mjs":
    case "cjs":
      return "JS";
    case "ts":
    case "tsx":
      return "TS";
    case "jsx":
      return "JSX";
    case "json":
      return "{}";
    case "py":
      return "PY";
    case "rs":
      return "RS";
    case "go":
      return "GO";
    default:
      return ext.slice(0, 3).toUpperCase();
  }
}
