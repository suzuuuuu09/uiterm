import { describe, expect, it } from "vite-plus/test";
import { t } from "../../i18n/dict.ts";
import { pocStore } from "../../store/pocStore.ts";
import { MockHerdrTransport } from "../../transport/mockHerdrTransport.ts";
import { validateResizePayload } from "../../transport/schemas.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

describe("Phase 0 Frontend PoC core contracts", () => {
  it("validates resize payloads with Valibot", () => {
    expect(validateResizePayload({ sessionId: "term-1", cols: 120, rows: 40 })).toEqual({
      sessionId: "term-1",
      cols: 120,
      rows: 40,
    });
    expect(() => validateResizePayload({ sessionId: "term-1", cols: 0, rows: 24 })).toThrow();
  });

  it("resolves TypeScript dictionary i18n keys without i18next", () => {
    expect(t("ja", "appTitle")).toBe("uiterm Phase 0 PoC");
    expect(t("ja", "backend")).toBe("描画エンジン");
    expect(t("en", "backend")).toBe("Renderer");
  });

  it("pipes binary Uint8Array stream directly without mutating Zustand store with output", () => {
    const transport = new MockHerdrTransport("test-1");
    const chunks: Uint8Array[] = [];
    const unsub = transport.subscribeOutput((chunk) => {
      chunks.push(chunk.bytes);
    });

    transport.attach(80, 24);
    expect(chunks.length).toBe(1);
    expect(decoder.decode(chunks[0]!)).toContain("herdr-core mock");

    // Verify Japanese UTF-8 input echo
    transport.sendInput(encoder.encode("日本語テスト\r"));
    const combined = chunks.map((c) => decoder.decode(c)).join("");
    expect(combined).toContain("日本語テスト");

    // Verify resync
    transport.requestResync();
    expect(decoder.decode(chunks[chunks.length - 1]!)).toContain("herdr-core resync");

    // Verify Zustand store has no terminal output buffer field
    expect("terminalOutput" in pocStore.getState()).toBe(false);

    unsub();
    transport.dispose();
  });

  it("generates 10,000-line binary burst for benchmarking", () => {
    const transport = new MockHerdrTransport("bench-1");
    let receivedBytes = 0;
    transport.subscribeOutput((chunk) => {
      receivedBytes += chunk.bytes.byteLength;
    });

    const result = transport.emitBurst(10_000, 200);
    expect(result.totalBytes).toBeGreaterThan(800_000);
    expect(receivedBytes).toBe(result.totalBytes);
    transport.dispose();
  });
});
