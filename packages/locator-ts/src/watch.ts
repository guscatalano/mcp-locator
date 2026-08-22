import { watch, type FSWatcher } from 'node:fs';
import type { Catalog } from './types.js';
import { enumerate, type EnumerateOptions } from './catalog.js';
import { resolveRoots } from './dirs.js';

export interface WatchOptions extends EnumerateOptions {
  /** Coalesce bursts of filesystem events; installers write several files at once. */
  debounceMs?: number;
}

export interface CatalogWatcher {
  close(): void;
}

/**
 * Watch every registry directory and re-enumerate on change. Uses node:fs watch rather than a
 * watcher dependency — this library gets embedded in AI clients, so its dependency surface is
 * kept to the schema validator alone.
 */
export function watchCatalog(
  options: WatchOptions,
  onChange: (catalog: Catalog) => void,
): CatalogWatcher {
  const env = options.env ?? process.env;
  const roots = options.roots ?? resolveRoots(env, options.platform ?? process.platform);
  const debounceMs = options.debounceMs ?? 150;

  const watchers: FSWatcher[] = [];
  let timer: NodeJS.Timeout | undefined;
  let closed = false;

  const fire = () => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      if (!closed) onChange(enumerate({ ...options, roots }));
    }, debounceMs);
  };

  for (const root of roots) {
    try {
      const w = watch(root.path, { persistent: false }, fire);
      w.on('error', () => {}); // directory removed underneath us; other roots keep working
      watchers.push(w);
    } catch {
      // A registry directory that does not exist yet is normal — nothing to watch.
    }
  }

  return {
    close() {
      closed = true;
      if (timer) clearTimeout(timer);
      for (const w of watchers) w.close();
    },
  };
}
