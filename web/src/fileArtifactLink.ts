const ARTIFACT_EXTENSIONS = new Set([
  "md", "markdown", "js", "mjs", "cjs", "ts", "tsx", "jsx", "json", "py", "rs", "go", "css", "scss", "html", "htm",
  "yaml", "yml", "sh", "bash", "zsh", "sql", "toml", "xml", "svg", "vue", "svelte",
]);

export type FileArtifactRef = { name: string; path: string; ext: string };

export function fileArtifactFromHref(href: string, label: string): FileArtifactRef | null {
  const trimmed = href.trim();
  if (!trimmed || /^https?:/i.test(trimmed) || trimmed.startsWith("mailto:") || trimmed.startsWith("#")) return null;
  const path = trimmed.replace(/^file:\/\//, "");
  const name = (path.split(/[/\\]/).pop() || label).trim();
  if (!name) return null;
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return null;
  const ext = name.slice(dot + 1).toLowerCase();
  if (!ARTIFACT_EXTENSIONS.has(ext)) return null;
  return { name, path, ext };
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
