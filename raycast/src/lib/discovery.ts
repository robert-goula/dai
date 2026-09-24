import { join } from "node:path";

export const DEFAULT_PORT = 4747;

/**
 * DAI's data dir. Mirrors `directories::ProjectDirs::from("", "", "dai").data_dir()`,
 * with the "DAI data folder" preference standing in for `$DAI_HOME`.
 */
export function dataDir(opts: { platform: NodeJS.Platform; homedir: string; override?: string }): string {
  if (opts.override) return opts.override;
  if (opts.platform === "darwin") return join(opts.homedir, "Library", "Application Support", "dai");
  throw new Error(`unsupported platform: ${opts.platform}`);
}

/** Port from `daemon.json`'s contents; the default when missing or malformed. */
export function parsePort(json: string | undefined): number {
  if (json === undefined) return DEFAULT_PORT;
  try {
    const port: unknown = JSON.parse(json)?.port;
    return typeof port === "number" && Number.isInteger(port) && port > 0 && port < 65536 ? port : DEFAULT_PORT;
  } catch {
    return DEFAULT_PORT;
  }
}
