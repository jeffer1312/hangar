export interface FileItem { fileName: string; filePath: string; fullPath: string; fileType: 'file' | 'folder'; }
export function searchFiles(_query: string, _opts?: unknown): FileItem[] { return []; }
