const ESC = 0x1b;
const MODE = 2048;
const BEL = 0x07;
const PRESERVE_PROMPT = new TextEncoder().encode(";redraw=0");

export class TerminalOutputProtocol {
  enabled = false;
  private savedEnabled = false;
  private state: "ground" | "escape" | "csi" | "private" | "osc" | "string" = "ground";
  private param = 0;
  private matched = false;
  private oscField = "";
  private oscFieldIndex = 0;
  private oscPrompt = false;
  private oscExplicitRedraw = false;

  /** Normalizes prompt markers and counts in-band resize requests across output chunks. */
  process(bytes: Uint8Array): { bytes: Uint8Array; resizeRequests: number } {
    let requests = 0;
    const insertions: number[] = [];
    for (let index = 0; index < bytes.length; index++) {
      if (this.state === "ground") {
        index = bytes.indexOf(ESC, index);
        if (index < 0) break;
        this.state = "escape";
        continue;
      }

      const byte = bytes[index];
      if (this.state === "osc" && (byte === BEL || byte === ESC)) {
        this.finishOscField();
        // Ghostty clears prompts on resize by default. Keep them unless the
        // shell explicitly promises to repaint.
        if (this.oscPrompt && this.oscFieldIndex === 2 && !this.oscExplicitRedraw) {
          insertions.push(index);
        }
        this.state = byte === ESC ? "escape" : "ground";
        continue;
      }

      if (byte === ESC || byte === 0x18 || byte === 0x1a) {
        this.state = byte === ESC ? "escape" : "ground";
        continue;
      }

      // C0 controls and DEL do not end an ESC or CSI sequence.
      if ((byte < 0x20 || byte === 0x7f) &&
          (this.state === "escape" || this.state === "csi" || this.state === "private")) {
        continue;
      }

      switch (this.state) {
        case "escape":
          this.state = "ground";
          if (byte === 0x5b) this.state = "csi";
          else if (byte === 0x5d) {
            this.state = "osc";
            this.oscField = "";
            this.oscFieldIndex = 0;
            this.oscPrompt = true;
            this.oscExplicitRedraw = false;
          } else if (byte === 0x50 || byte === 0x58 || byte === 0x5e || byte === 0x5f) {
            this.state = "string";
          } else if (byte === 0x63) {
            this.enabled = false;
            this.savedEnabled = false;
          }
          break;
        case "csi":
          this.state = byte === 0x3f ? "private" : "ground";
          this.param = 0;
          this.matched = false;
          break;
        case "private":
          if (byte >= 0x30 && byte <= 0x39) {
            this.param = Math.min(MODE + 1, this.param * 10 + byte - 0x30);
          } else if (byte === 0x3b) {
            this.matched ||= this.param === MODE;
            this.param = 0;
          } else if (byte >= 0x20 && byte !== 0x7f) {
            if (this.matched || this.param === MODE) {
              if (byte === 0x68) {
                this.enabled = true;
                requests++;
              } else if (byte === 0x6c) {
                this.enabled = false;
              } else if (byte === 0x73) {
                this.savedEnabled = this.enabled;
              } else if (byte === 0x72) {
                this.enabled = this.savedEnabled;
              }
            }
            this.state = "ground";
          }
          break;
        case "osc":
          if (byte === 0x3b) this.finishOscField();
          else if (this.oscPrompt && this.oscField.length < "redraw=".length) {
            this.oscField += String.fromCharCode(byte);
          }
          break;
        case "string":
          break;
      }
    }
    if (insertions.length === 0) return { bytes, resizeRequests: requests };

    const normalized = new Uint8Array(bytes.length + insertions.length * PRESERVE_PROMPT.length);
    let offset = 0;
    let start = 0;
    for (const end of insertions) {
      normalized.set(bytes.subarray(start, end), offset);
      offset += end - start;
      normalized.set(PRESERVE_PROMPT, offset);
      offset += PRESERVE_PROMPT.length;
      start = end;
    }
    normalized.set(bytes.subarray(start), offset);
    return { bytes: normalized, resizeRequests: requests };
  }

  private finishOscField(): void {
    if (!this.oscPrompt) return;

    if (this.oscFieldIndex === 0) this.oscPrompt = this.oscField === "133";
    else if (this.oscFieldIndex === 1) this.oscPrompt = this.oscField === "A" || this.oscField === "N";
    else this.oscExplicitRedraw ||= this.oscField === "redraw=";

    this.oscFieldIndex = Math.min(2, this.oscFieldIndex + 1);
    this.oscField = "";
  }
}
