export declare const BUNDLE_FILES: readonly string[];
export declare function bundle(
  sources: Record<string, string>,
  meta: { version: string; protocol: number; source: string },
): Promise<string>;
