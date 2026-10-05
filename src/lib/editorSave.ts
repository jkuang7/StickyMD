export function createEditorSaveQueue(
  invoke: (command: string, args: Record<string, unknown>) => Promise<unknown>,
) {
  let chain: Promise<void> = Promise.resolve();
  return function save(
    document: unknown,
    color?: string,
    isCurrent: () => boolean = () => true,
  ): Promise<void> {
    const args = color === undefined ? { document } : { document, color };
    const next = chain.catch(() => undefined).then(async () => {
      if (isCurrent()) await invoke("save_note", args);
    });
    chain = next;
    return next;
  };
}
