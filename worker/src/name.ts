// Which folder names may be asked about: the same rule as the app's
// (mdif_core::knowledge::generic), checked again here, since anything can
// send anything. Only names that say nothing about the person: an app's
// folders, a tool's, macOS's; never their own folders, never what's inside.

/** Folders an app keeps in, by its bundle id. */
const APP_KINDS = ["Containers", "Group Containers", "Application Support", "Caches"];
/** Folders of the person's own, asked about only as a whole. */
const YOURS = ["CloudStorage", "Mobile Documents", "Mail", "Messages"];
/** Names that say nothing but "data" when they follow an app's or tool's. */
const STANDARD = [
  "Data", "Library", "Caches", "Cache", "Application Support", "tmp", "cache", "caches",
  "install", "registry", "bin", "lib", "share", "pkg", "mod",
];

/** A name made for one Mac (a UUID, a long number) would tell it apart. */
export function identifies(name: string): boolean {
  return /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i.test(name)
    || /[0-9a-f]{16}/i.test(name)
    || /[0-9]{6}/.test(name);
}

function standard(rest: string[]): number {
  let n = 0;
  while (n < rest.length && STANDARD.includes(rest[n])) n++;
  return n;
}

/** Whether `name` is one the app would ask about, exactly as it would say it. */
export function askable(name: string): boolean {
  if (name.length < 3 || name.length > 200 || name !== name.normalize("NFC")) return false;
  if (/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(name)) return false;
  const home = name.startsWith("~/");
  if (!home && !name.startsWith("/")) return false;
  const n = name.slice(home ? 2 : 1).split("/");
  if (n.some((c) => c === "" || c === "." || c === ".." || c.length > 100 || identifies(c))) {
    return false;
  }
  if (!home) return n.length <= 3 && n[0] !== "Users" && n[0] !== "Volumes";
  const [first, kind] = n;
  if (first === "Library") {
    if (APP_KINDS.includes(kind) && n.length >= 3) return 3 + standard(n.slice(3)) === n.length;
    if (YOURS.includes(kind)) return n.length === 2;
    return n.length <= (kind === "Developer" ? 4 : 3);
  }
  if (first.startsWith(".")) {
    const tool = first === ".local" ? 2 : first === ".cache" || first === ".config" ? 1 : 0;
    const rest = n.slice(1);
    return rest.length <= tool || tool + standard(rest.slice(tool)) === rest.length;
  }
  return false;
}
