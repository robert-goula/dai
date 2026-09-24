import type { ProjectReport } from "./api";

/** One-line status for the Search tab, plus details for a tooltip. */
export function projectStatus(r: ProjectReport): { summary: string; details: string[] } {
  const details: string[] = [];
  let matched = 0;
  let mismatched = 0;
  for (const d of r.covered) {
    const version = d.dependency.resolved ?? d.dependency.spec;
    const label = version ? `${d.dependency.name} ${version}` : d.dependency.name;
    const best = d.installed[0];
    if (!best) continue;
    if (best.matches === false) {
      mismatched++;
      const fix = r.suggestions.find((s) => s.dependency === d.dependency.name);
      details.push(
        `✗ ${label}: only ${best.id} (${best.version})${fix ? ` — install ${fix.id}` : ""}`,
      );
    } else {
      matched++;
      details.push(`✓ ${label}: ${best.id}`);
    }
  }
  for (const s of r.suggestions) {
    if (!r.covered.some((d) => d.dependency.name === s.dependency)) {
      details.push(`+ ${s.dependency}: available as ${s.id}`);
    }
  }
  const parts = [`${matched} matched`];
  if (mismatched) parts.push(`${mismatched} other version`);
  const available = r.suggestions.length;
  if (available) parts.push(`${available} to install`);
  return { summary: parts.join(" · "), details };
}

export function folderName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}
