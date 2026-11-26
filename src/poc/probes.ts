/**
 * Phase 0 PoC diagnostic payloads for Japanese width/IME, VT/ANSI, auto-reply probing,
 * and high-throughput burst benchmarking.
 */
export const JAPANESE_TEST_PAYLOAD = [
  "\r\n\x1b[1;36m=== 日本語・文字幅・フォントフォールバック検証 ===\x1b[0m\r\n",
  "ASCII 40 cols : 1234567890123456789012345678901234567890|\r\n",
  "全角 20文字   : あいうえおかきくけこさしすせそたちつてと|\r\n",
  "混在テキスト  : Hello日本語World1234漢字ABCあいうえお  |\r\n",
  "曖昧幅(East_A): ○●△▲□■☆★※→←↑↓─│┌┐└┘┼|\r\n",
  "半角カナ      : ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄ12345678901234567890|\r\n",
  "絵文字/ZWJ    : 🚀 🎉 👨‍💻 🇯🇵 👨‍👩‍👧‍👦 👍🏽 ✨\r\n",
  "結合文字(NFD) : か\u3099 き\u3099 く\u3099 e\u0301 a\u0308 (濁点結合・アクセント)\r\n",
  "罫線ボックス  : ┌────────────────────────┐\r\n",
  "              : │ 日本語テキスト整列確認 │\r\n",
  "              : └────────────────────────┘\r\n",
  "日本語IME入力テスト → ここに直接入力してください: ",
].join("");

export const VT_ANSI_TEST_PAYLOAD = [
  "\r\n\x1b[1;35m=== VT / ANSI 互換性テスト ===\x1b[0m\r\n",
  "SGR: \x1b[1mBold\x1b[0m \x1b[2mDim\x1b[0m \x1b[3mItalic\x1b[0m \x1b[4mUnderline\x1b[0m \x1b[9mStrike\x1b[0m \x1b[7mInverse\x1b[0m\r\n",
  "TrueColor: \x1b[38;2;255;100;100mR\x1b[38;2;100;255;100mG\x1b[38;2;100;160;255mB\x1b[0m ",
  "\x1b[48;2;30;60;120;38;2;255;255;255m 24-bit BG+FG \x1b[0m\r\n",
  "256-color: ",
  Array.from({ length: 16 }, (_, i) => `\x1b[48;5;${16 + i * 12}m  \x1b[0m`).join(""),
  "\r\n",
  "OSC 8 Link: \x1b]8;;https://ghostty.org\x07Ghostty\x1b]8;;\x07\r\n",
  "OSC 7501 (herdr): \x1b]7501;state=working:app=codex\x1b\\(status emitted)\r\n",
  "\x1b[32muiterm-mock\x1b[0m$ ",
].join("");

export const VT_QUERY_TEST_PAYLOAD =
  "\r\n\x1b[33m[Auto-Reply Probe]\x1b[0m Sending DA1 (CSI c), DA2 (CSI > c), DSR (CSI 6 n), OSC 11 ? ...\r\n" +
  "\x1b[c\x1b[>c\x1b[6n\x1b]11;?\x07" +
  "\x1b[32muiterm-mock\x1b[0m$ ";
