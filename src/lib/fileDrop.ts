export type SendTerminalInput = (id: string, data: string) => Promise<boolean>;

export function quoteShellPath(path: string): string {
  return `'${path.split("'").join("'\\''")}'`;
}

export function droppedFileInput(paths: readonly string[]): string | null {
  if (paths.length === 0) return null;
  return `${paths.map(quoteShellPath).join(" ")} `;
}

export async function insertDroppedFilesIntoActiveTerminal(
  activeId: string | null,
  paths: readonly string[],
  sendTerminalInput: SendTerminalInput,
): Promise<boolean> {
  if (activeId === null) return false;
  const input = droppedFileInput(paths);
  if (input === null) return false;
  return sendTerminalInput(activeId, input);
}
