import type { Snapshot } from "./contracts";

// A write arriving during a read requires another read before callers can continue.
export class ConfigurationReader {
  value: Snapshot | null = null;
  private reading: Promise<void> | null = null;
  private generation = 0;
  private read: () => Promise<Snapshot>;
  private render: (value: Snapshot) => void;
  constructor(
    read: () => Promise<Snapshot>,
    render: (value: Snapshot) => void,
  ) {
    this.read = read;
    this.render = render;
  }
  refresh(): Promise<void> {
    this.generation++;
    if (this.reading) return this.reading;
    this.reading = (async () => {
      let requested: number;
      do {
        requested = this.generation;
        const next = await this.read();
        if (requested === this.generation) {
          this.value = next;
          this.render(next);
        }
      } while (requested !== this.generation);
    })().finally(() => {
      this.reading = null;
    });
    return this.reading;
  }
  updateSession(
    next: Omit<Snapshot, "settings"> & {
      revisions: number[];
      startup: Snapshot["settings"]["startup"];
    },
  ) {
    if (!this.value || this.reading) return false;
    const s = this.value.settings;
    if (
      [
        s.nodesRevision,
        s.routingRevision,
        s.coreRevision,
        s.preferencesRevision,
        s.connectionRevision,
      ].some((v, i) => v !== next.revisions[i])
    )
      return false;
    this.value = {
      ...this.value,
      ...next,
      settings: { ...s, startup: next.startup },
    };
    this.render(this.value);
    return true;
  }
}
